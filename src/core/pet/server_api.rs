//! Nacho Pet ↔ nacho-server REST calls the pet makes besides the chat socket. Port of `PetServerAPI.swift`.
//! Contract (nacho-server, see [`PET_SERVER_ROUTES`]):
//!   GET  /api/v1/rooms/{id}/history?view=app&limit&before_seq   → Message[] + header X-Has-More   (spec/rooms.tsp)
//!   GET  /api/v1/rooms/{id}/delta?view=app&since_seq            → RoomDelta {messages,last_seq,read_seq,has_more}
//!   read mark: WS frame {type:read,room_id,seq} (`ChatClient::mark_read`)
//!   GET  /api/v1/me/usage                                       → UsageState                      (spec/usage.tsp)
//!   GET  /client/config  (no token)                             → ClientConfig                    (spec/client-config.tsp)
//!   POST /api/v1/dictation  multipart file/language/duration_sec → {text,language,duration_sec?}   (spec/dictation.tsp)
//!   POST /api/v1/memory/feedback {text,kind,memory_ids,room_id} + Idempotency-Key → 201 FeedbackOutcome (spec/memory.tsp)
//!   DELETE /api/v1/memory/facts|episodes/{id} → 204;  POST /api/v1/memory/{id}/restore → 200 | 410 restore_expired
//!   POST /api/v1/chat/requests/{id}/answer {action,option_id?,option_ids?,expected_version} → CardRequest (spec/cards.tsp)
//!   GET  /api/v1/tasks/{id}, POST /api/v1/tasks/{id}/start {expected_estimate_id}, POST /api/v1/tasks/{id}/cancel → Task
//! Errors: {"error":{"code","message","reason?"}} (docs/error-codes.md) → ErrorText.
use super::chat_state::ErrorText;
use super::frames::ActivityKind;
use super::http::{PetHTTPClient, PetHTTPRequest, PetHTTPResponse, PetServerError};
use super::json::{self, JsonExt};
use super::media::PetIDs;
use super::server_models::{ChatMessage, PetCard, PetClientConfig, PetTask, PetUsage};
use super::settings::ServerEndpoint;
use serde_json::{json, Value};
use std::sync::Arc;

/// Every REST route the pet calls, as nacho-server's spec names it. One route per line (`    "METHOD /path",`) so a
/// script can pull the list out with sed and check it against a newer server checkout; a test checks that every
/// request the pet builds is in it and that nothing listed is unused.
pub const PET_SERVER_ROUTES: &[&str] = &[
    "GET /client/config",
    "GET /api/v1/me",
    "GET /api/v1/me/usage",
    "GET /api/v1/connections",
    "GET /api/v1/tools",
    "POST /api/v1/media/uploads",
    "POST /api/v1/agents/{id}/connector-binding",
    "GET /api/v1/rooms/{id}/history",
    "GET /api/v1/rooms/{id}/delta",
    "POST /api/v1/dictation",
    "POST /api/v1/memory/feedback",
    "DELETE /api/v1/memory/facts/{id}",
    "DELETE /api/v1/memory/episodes/{id}",
    "POST /api/v1/memory/{id}/restore",
    "GET /api/v1/chat/requests/{id}",
    "POST /api/v1/chat/requests/{id}/answer",
    "GET /api/v1/tasks/{id}",
    "POST /api/v1/tasks/{id}/start",
    "POST /api/v1/tasks/{id}/cancel",
];

/// The server spec the route list was last checked against.
pub const PET_SERVER_SPEC_REF: &str = "nacho-server origin/main 2ff6d8c (2026-11-15)";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetAPIError {
    /// 0 = no answer (network).
    pub status: u16,
    pub code: Option<String>,
    /// Korean, for the user.
    pub text: String,
}

impl PetAPIError {
    pub fn network() -> Self { Self { status: 0, code: None, text: "서버에 연결하지 못했어요. 네트워크를 확인해 주세요.".into() } }
    pub fn bad_response() -> Self { Self { status: 0, code: Some("bad_response".into()), text: "서버 응답을 읽지 못했어요.".into() } }

    pub fn from_response(resp: &PetHTTPResponse) -> Self {
        Self { status: resp.status, code: PetServerError::code(&resp.body), text: ErrorText::from_body(&resp.body, resp.status) }
    }

    pub fn is_unauthorized(&self) -> bool { self.status == 401 || self.code.as_deref() == Some("unauthorized") }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PetHistoryPage {
    pub messages: Vec<ChatMessage>,
    pub has_more: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PetDelta {
    pub messages: Vec<ChatMessage>,
    pub last_seq: i64,
    pub read_seq: i64,
    pub has_more: bool,
    pub activity: Option<ActivityKind>,
}

/// The answer to `POST /tasks/{id}/start`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PetTaskStart {
    /// Started (running / queued), or the task's current state when it was not planned any more.
    Started(PetTask),
    /// Not started: the estimate changed since the user saw it; the task carries the new one.
    EstimateChanged(PetTask),
}

pub struct PetServerAPI;

impl PetServerAPI {
    pub const HISTORY_PAGE_SIZE: i64 = 50;

    /// Audio types the server takes (spec/dictation.tsp DictationUpload.file).
    pub const DICTATION_TYPES: [&'static str; 7] = ["audio/mp4", "audio/m4a", "audio/x-m4a", "audio/aac", "audio/wav", "audio/webm", "audio/mpeg"];

    /// `base + path` with a query (names and values percent-encoded).
    pub fn url(e: &ServerEndpoint, path: &str, query: &[(&str, String)]) -> String {
        if query.is_empty() { return e.rest(path); }
        let q: Vec<String> = query.iter().map(|(k, v)| format!("{}={}", PetHTTPRequest::form_escape(k), PetHTTPRequest::form_escape(v))).collect();
        format!("{}?{}", e.rest(path), q.join("&"))
    }

    /// An id as one path segment (ids are UUIDs; anything else cannot leave its segment).
    fn seg(id: &str) -> String { PetHTTPRequest::form_escape(id) }

    // MARK: requests (pure; tested against the contract)

    pub fn history_request(e: &ServerEndpoint, bearer: Option<&str>, room_id: &str, before_seq: Option<i64>, limit: i64) -> PetHTTPRequest {
        let mut q = vec![("view", "app".to_string()), ("limit", limit.to_string())];
        if let Some(b) = before_seq { q.push(("before_seq", b.to_string())); }
        PetHTTPRequest::get(Self::url(e, &format!("/api/v1/rooms/{}/history", Self::seg(room_id)), &q), bearer)
    }

    pub fn delta_request(e: &ServerEndpoint, bearer: Option<&str>, room_id: &str, since_seq: i64) -> PetHTTPRequest {
        let q = [("view", "app".to_string()), ("since_seq", since_seq.to_string())];
        PetHTTPRequest::get(Self::url(e, &format!("/api/v1/rooms/{}/delta", Self::seg(room_id)), &q), bearer)
    }

    pub fn usage_request(e: &ServerEndpoint, bearer: Option<&str>) -> PetHTTPRequest { PetHTTPRequest::get(e.rest("/api/v1/me/usage"), bearer) }

    /// No token: the config is public.
    pub fn client_config_request(e: &ServerEndpoint) -> PetHTTPRequest { PetHTTPRequest::get(e.rest("/client/config"), None) }

    /// Multipart: `language`, `duration_sec` (2 decimals), then `file` (the audio, with its Content-Type).
    #[allow(clippy::too_many_arguments)]
    pub fn dictation_request(
        e: &ServerEndpoint,
        bearer: Option<&str>,
        audio: &[u8],
        mime: &str,
        file_name: &str,
        language: &str,
        duration_sec: Option<f64>,
        boundary: &str,
    ) -> PetHTTPRequest {
        let mut body: Vec<u8> = vec![];
        let mut part = |name: &str, value: &str| {
            body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
        };
        part("language", language);
        if let Some(d) = duration_sec { part("duration_sec", &format!("{d:.2}")); }
        // The file name goes into a quoted header value: quotes and line breaks cannot be part of it.
        let safe_name: String = file_name.chars().filter(|c| !matches!(c, '"' | '\r' | '\n')).collect();
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{safe_name}\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(audio);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let mut r = PetHTTPRequest::get(e.rest("/api/v1/dictation"), bearer);
        r.method = "POST".into();
        r.headers.insert("Content-Type".into(), format!("multipart/form-data; boundary={boundary}"));
        r.body = Some(body);
        r
    }

    /// "이 기억이 틀렸어요": a correction about the memories an answer used.
    #[allow(clippy::too_many_arguments)]
    pub fn feedback_request(
        e: &ServerEndpoint,
        bearer: Option<&str>,
        text: &str,
        kind: &str,
        memory_ids: &[String],
        room_id: Option<&str>,
        agent_id: Option<&str>,
        idempotency_key: &str,
    ) -> PetHTTPRequest {
        let mut j = json!({"text": text, "kind": kind});
        if !memory_ids.is_empty() { j["memory_ids"] = json!(memory_ids); }
        if let Some(r) = room_id { j["room_id"] = json!(r); }
        if let Some(a) = agent_id { j["agent_id"] = json!(a); }
        let mut r = PetHTTPRequest::post_json(e.rest("/api/v1/memory/feedback"), bearer, &j);
        r.headers.insert("Idempotency-Key".into(), idempotency_key.into());
        r
    }

    pub fn forget_request(e: &ServerEndpoint, bearer: Option<&str>, memory_id: &str, kind: &str) -> PetHTTPRequest {
        let what = if kind == "episode" { "episodes" } else { "facts" };
        let mut r = PetHTTPRequest::get(e.rest(&format!("/api/v1/memory/{what}/{}", Self::seg(memory_id))), bearer);
        r.method = "DELETE".into();
        r
    }

    pub fn restore_request(e: &ServerEndpoint, bearer: Option<&str>, memory_id: &str) -> PetHTTPRequest {
        PetHTTPRequest::post_json(e.rest(&format!("/api/v1/memory/{}/restore", Self::seg(memory_id))), bearer, &json!({}))
    }

    /// `GET /api/v1/chat/requests/{id}` → CardRequest (a card's current state after a version conflict).
    pub fn card_request(e: &ServerEndpoint, bearer: Option<&str>, card_id: &str) -> PetHTTPRequest {
        PetHTTPRequest::get(e.rest(&format!("/api/v1/chat/requests/{}", Self::seg(card_id))), bearer)
    }

    /// approval: approve | deny · choice/suggestions: choose (+ option_id / option_ids) · any: dismiss.
    pub fn answer_card_request(
        e: &ServerEndpoint,
        bearer: Option<&str>,
        card: &PetCard,
        action: &str,
        option_ids: &[String],
        allow_rule_id: Option<&str>,
    ) -> PetHTTPRequest {
        let mut j = json!({"action": action, "expected_version": card.version});
        // `allow_rule` only with approve (the server answers 400 otherwise); the server adds this Nacho itself.
        if let Some(rule) = allow_rule_id.filter(|r| action == "approve" && !r.is_empty()) { j["allow_rule"] = json!({"rule_option_id": rule}); }
        if card.multiple && !option_ids.is_empty() {
            j["option_ids"] = json!(option_ids);
        } else if let Some(o) = option_ids.first() {
            j["option_id"] = json!(o);
        }
        PetHTTPRequest::post_json(e.rest(&format!("/api/v1/chat/requests/{}/answer", Self::seg(&card.id))), bearer, &j)
    }

    /// `GET /api/v1/tasks/{id}` → Task (spec/tasks.tsp).
    pub fn task_request(e: &ServerEndpoint, bearer: Option<&str>, task_id: &str) -> PetHTTPRequest {
        PetHTTPRequest::get(e.rest(&format!("/api/v1/tasks/{}", Self::seg(task_id))), bearer)
    }

    /// `POST /api/v1/tasks/{id}/start {expected_estimate_id}` → Task, or 409 `estimate_changed` with the task carrying the
    /// new estimate. Not planned → the current task (idempotent).
    pub fn start_task_request(e: &ServerEndpoint, bearer: Option<&str>, task_id: &str, expected_estimate_id: Option<&str>) -> PetHTTPRequest {
        let mut j = json!({});
        if let Some(id) = expected_estimate_id { j["expected_estimate_id"] = json!(id); }
        PetHTTPRequest::post_json(e.rest(&format!("/api/v1/tasks/{}/start", Self::seg(task_id))), bearer, &j)
    }

    /// `POST /api/v1/tasks/{id}/cancel` → Task (canceled; already ended → the current task).
    pub fn cancel_task_request(e: &ServerEndpoint, bearer: Option<&str>, task_id: &str) -> PetHTTPRequest {
        PetHTTPRequest::post_json(e.rest(&format!("/api/v1/tasks/{}/cancel", Self::seg(task_id))), bearer, &json!({}))
    }

    // MARK: decoding

    pub fn decode_history(resp: &PetHTTPResponse) -> Option<PetHistoryPage> {
        let Value::Array(arr) = serde_json::from_slice::<Value>(&resp.body).ok()? else { return None };
        let messages = arr.iter().filter_map(Value::as_object).map(ChatMessage::decode).collect();
        Some(PetHistoryPage { messages, has_more: resp.header("X-Has-More").is_some_and(|v| v.eq_ignore_ascii_case("true")) })
    }

    pub fn decode_delta(data: &[u8]) -> Option<PetDelta> {
        let o = json::object(data)?;
        let msgs = o.get("messages")?.as_array()?;
        Some(PetDelta {
            messages: msgs.iter().filter_map(Value::as_object).map(ChatMessage::decode).collect(),
            last_seq: o.int("last_seq").unwrap_or(0),
            read_seq: o.int("read_seq").unwrap_or(0),
            has_more: o.bool("has_more").unwrap_or(false),
            activity: o.obj("activity").and_then(|a| a.str("kind")).and_then(|k| ActivityKind::from_raw(&k)),
        })
    }

    pub fn decode_usage(data: &[u8]) -> Option<PetUsage> { PetUsage::decode_data(data) }

    pub fn decode_dictation(data: &[u8]) -> Option<String> { json::object(data)?.str("text") }

    pub fn decode_card(data: &[u8]) -> Option<PetCard> { json::object(data).map(|o| PetCard::decode(&o)) }
}

pub type APICompletion<T> = Box<dyn FnOnce(Result<T, PetAPIError>) + Send>;

/// Runs the requests with a bearer token and maps the answers; completions on any thread.
#[derive(Clone)]
pub struct PetServerCalls {
    pub endpoint: ServerEndpoint,
    pub http: Arc<dyn PetHTTPClient>,
    pub bearer: Option<String>,
}

impl PetServerCalls {
    pub fn new(endpoint: ServerEndpoint, http: Arc<dyn PetHTTPClient>, bearer: Option<String>) -> Self { Self { endpoint, http, bearer } }

    fn bearer(&self) -> Option<&str> { self.bearer.as_deref() }

    /// 2xx → `decode` (None = bad response); anything else → the server's error in Korean.
    fn run<T: 'static>(&self, req: PetHTTPRequest, decode: impl FnOnce(&PetHTTPResponse) -> Option<T> + Send + 'static, completion: APICompletion<T>) {
        self.http.send(
            req,
            Box::new(move |r| match r {
                Err(_) => completion(Err(PetAPIError::network())),
                Ok(resp) if !resp.is_success() => completion(Err(PetAPIError::from_response(&resp))),
                Ok(resp) => completion(decode(&resp).ok_or_else(PetAPIError::bad_response)),
            }),
        );
    }

    pub fn history(&self, room_id: &str, before_seq: Option<i64>, completion: APICompletion<PetHistoryPage>) {
        let req = PetServerAPI::history_request(&self.endpoint, self.bearer(), room_id, before_seq, PetServerAPI::HISTORY_PAGE_SIZE);
        self.run(req, PetServerAPI::decode_history, completion);
    }

    /// Everything since `since_seq` (follows `has_more`, at most 5 pages = 2500 messages).
    pub fn delta(&self, room_id: &str, since_seq: i64, completion: APICompletion<PetDelta>) { self.delta_pages(room_id, since_seq, 5, vec![], completion) }

    pub fn delta_pages(&self, room_id: &str, since_seq: i64, pages: u32, collected: Vec<ChatMessage>, completion: APICompletion<PetDelta>) {
        let me = self.clone();
        let room = room_id.to_string();
        let req = PetServerAPI::delta_request(&self.endpoint, self.bearer(), room_id, since_seq);
        self.run(
            req,
            |resp| PetServerAPI::decode_delta(&resp.body),
            Box::new(move |r: Result<PetDelta, PetAPIError>| match r {
                Err(e) => completion(Err(e)),
                Ok(mut d) => {
                    let mut all = collected;
                    all.append(&mut d.messages);
                    d.messages = all;
                    let last = d.messages.iter().filter_map(|m| m.seq).max();
                    match last {
                        Some(last) if d.has_more && pages > 1 && d.last_seq > since_seq && last > since_seq => {
                            me.delta_pages(&room, last, pages - 1, d.messages, completion)
                        }
                        _ => completion(Ok(d)),
                    }
                }
            }),
        );
    }

    pub fn usage(&self, completion: APICompletion<PetUsage>) {
        self.run(PetServerAPI::usage_request(&self.endpoint, self.bearer()), |r| PetServerAPI::decode_usage(&r.body), completion);
    }

    pub fn client_config(&self, completion: APICompletion<PetClientConfig>) {
        self.run(PetServerAPI::client_config_request(&self.endpoint), |r| PetClientConfig::decode(&r.body), completion);
    }

    /// `boundary` None = a fresh `nacho-<uuid>`. Language is "ko" for the pet.
    #[allow(clippy::too_many_arguments)]
    pub fn dictate(
        &self,
        audio: &[u8],
        mime: &str,
        file_name: &str,
        language: &str,
        duration_sec: Option<f64>,
        boundary: Option<&str>,
        completion: APICompletion<String>,
    ) {
        let b = boundary.map(str::to_string).unwrap_or_else(|| format!("nacho-{}", PetIDs::new_uuid()));
        let req = PetServerAPI::dictation_request(&self.endpoint, self.bearer(), audio, mime, file_name, language, duration_sec, &b);
        self.run(req, |r| PetServerAPI::decode_dictation(&r.body), completion);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn feedback(
        &self,
        text: &str,
        kind: &str,
        memory_ids: &[String],
        room_id: Option<&str>,
        agent_id: Option<&str>,
        idempotency_key: &str,
        completion: APICompletion<()>,
    ) {
        let req = PetServerAPI::feedback_request(&self.endpoint, self.bearer(), text, kind, memory_ids, room_id, agent_id, idempotency_key);
        self.run(req, |_| Some(()), completion);
    }

    pub fn forget(&self, memory_id: &str, kind: &str, completion: APICompletion<()>) {
        self.run(PetServerAPI::forget_request(&self.endpoint, self.bearer(), memory_id, kind), |_| Some(()), completion);
    }

    pub fn restore(&self, memory_id: &str, completion: APICompletion<()>) {
        self.run(PetServerAPI::restore_request(&self.endpoint, self.bearer(), memory_id), |_| Some(()), completion);
    }

    pub fn card(&self, card_id: &str, completion: APICompletion<PetCard>) {
        self.run(PetServerAPI::card_request(&self.endpoint, self.bearer(), card_id), |r| PetServerAPI::decode_card(&r.body), completion);
    }

    pub fn answer(&self, card: &PetCard, action: &str, option_ids: &[String], allow_rule_id: Option<&str>, completion: APICompletion<PetCard>) {
        let req = PetServerAPI::answer_card_request(&self.endpoint, self.bearer(), card, action, option_ids, allow_rule_id);
        self.run(req, |r| PetServerAPI::decode_card(&r.body), completion);
    }

    pub fn task(&self, id: &str, completion: APICompletion<PetTask>) {
        self.run(PetServerAPI::task_request(&self.endpoint, self.bearer(), id), |r| PetTask::decode_data(&r.body), completion);
    }

    /// Start a planned task. 409 `estimate_changed` is not an error here: it carries the task with the new estimate.
    pub fn start_task(&self, id: &str, expected_estimate_id: Option<&str>, completion: APICompletion<PetTaskStart>) {
        let req = PetServerAPI::start_task_request(&self.endpoint, self.bearer(), id, expected_estimate_id);
        self.http.send(req, Box::new(move |r| completion(Self::start_result(r))));
    }

    /// The answer to a start, as [`PetTaskStart`] or the error.
    pub fn start_result(r: super::http::HTTPResult) -> Result<PetTaskStart, PetAPIError> {
        let resp = r.map_err(|_| PetAPIError::network())?;
        if resp.is_success() { return PetTask::decode_data(&resp.body).map(PetTaskStart::Started).ok_or_else(PetAPIError::bad_response); }
        if resp.status == 409 && PetServerError::code(&resp.body).as_deref() == Some("estimate_changed") {
            if let Some(t) = json::object(&resp.body).and_then(|o| o.obj("task")) { return Ok(PetTaskStart::EstimateChanged(PetTask::decode(&t))); }
        }
        Err(PetAPIError::from_response(&resp))
    }

    pub fn cancel_task(&self, id: &str, completion: APICompletion<PetTask>) {
        self.run(PetServerAPI::cancel_task_request(&self.endpoint, self.bearer(), id), |r| PetTask::decode_data(&r.body), completion);
    }
}

#[cfg(test)]
mod tests {
    //! The non-session tests of `PetServerTests.swift`, `PetServerContractTests.swift`, the model/request tests of
    //! `PetTaskStartTests.swift` and testAnswerRequestCarriesAllowRuleOnlyWithApprove of `PetVoiceChoiceTests.swift`.
    use super::*;
    use crate::core::pet::chat_state::{ChatState, ToolStep, TurnStatus};
    use crate::core::pet::frames::{ServerEvent, ServerFrame};
    use crate::core::pet::http::testing::FakeHTTP;
    use crate::core::pet::http::PetNetError;
    use crate::core::pet::json::JsonObject;
    use crate::core::pet::media::PetMedia;
    use crate::core::pet::server_models::{PetMemoryItem, PetMemoryNote};
    use crate::core::pet::settings::URLParts;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Mutex;

    const ROOM: &str = "0193aaaa-0000-7000-8000-0000000000r1";

    fn frame(json: &str) -> ServerFrame { ServerFrame::decode(json).expect("frame") }
    fn obj(v: Value) -> JsonObject { v.as_object().cloned().unwrap() }

    fn msg(id: &str, seq: i64, role: &str, text: &str, client: Option<&str>, extra: &str) -> String {
        let c = client.map(|c| format!(r#","client_msg_id":"{c}""#)).unwrap_or_default();
        let sender = if role == "user" { "user" } else { "agent" };
        format!(
            r#"{{"id":"{id}","room_id":"{ROOM}","seq":{seq},"role":"{role}","sender_kind":"{sender}","text":"{text}","content":{{"type":"text","text":"{text}"}},"used_memory_ids":[],"redactions":[],"attachments":[]{c}{extra}}}"#
        )
    }

    fn decode_msg(s: &str) -> ChatMessage { ChatMessage::decode(&json::object_str(s).unwrap()) }

    fn ep(base: &str) -> ServerEndpoint { ServerEndpoint { base: base.into() } }

    fn path(r: &PetHTTPRequest) -> String { URLParts::split(&r.url).unwrap().0.path }

    fn query(r: &PetHTTPRequest) -> BTreeMap<String, String> {
        let Some((_, q)) = r.url.split_once('?') else { return BTreeMap::new() };
        q.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn body(r: &PetHTTPRequest) -> JsonObject { r.body_json().unwrap_or_default() }

    // MARK: PetServerFrameTests

    #[test]
    fn tool_card_task_memory_usage_notification_frames() {
        let started = frame(r#"{"type":"tool_call_started","room_id":"r","client_id":"c1","call_id":"k1","name":"search_web","arguments":{"q":"x"}}"#);
        assert_eq!(started.event, ServerEvent::ToolStarted { call_id: "k1".into(), name: "search_web".into() });
        assert_eq!(started.client_id.as_deref(), Some("c1"));
        let finished = frame(r#"{"type":"tool_call_finished","room_id":"r","client_id":"c1","call_id":"k1","name":"search_web","preview":"…","meta":{"source":"builtin","is_error":true,"duration_ms":120}}"#);
        assert_eq!(finished.event, ServerEvent::ToolFinished { call_id: "k1".into(), name: "search_web".into(), is_error: true, duration_ms: Some(120) });

        let card = frame(r#"{"type":"card_update","room_id":"r","request":{"id":"cd1","request_id":"cd1","kind":"choice","status":"pending","version":2,"payload":{"question":"어느 쪽?","options":[{"id":"a","label":"강남"},{"id":"b","label":"홍대"}]}}}"#);
        let ServerEvent::Card(c) = card.event else { panic!("card") };
        assert_eq!(c.title, "어느 쪽?");
        assert_eq!(c.options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), ["강남", "홍대"]);
        assert!(c.answerable_here());
        assert_eq!(c.version, 2);

        let approval = PetCard::decode(&obj(json!({"id": "x", "kind": "approval", "status": "pending", "version": 1,
            "payload": {"title": "예약", "amount": 30000, "merchant": "파스타집", "action_type": "payment"}})));
        // Approvals are answered here now (실행 / 거절).
        assert!(approval.answerable_here());
        assert_eq!(approval.status_text(), "실행할지 답해 주세요");
        assert_eq!(approval.detail.as_deref(), Some("파스타집 · 30,000원"));

        let task = frame(r#"{"type":"task_update","room_id":"r","task":{"id":"t1","state":"running","title":"파스타집 예약","current_step":{"text":"자리 찾는 중","index":2,"total":4}}}"#);
        let ServerEvent::Task(t) = task.event else { panic!("task") };
        assert_eq!(t.status_text(), "자리 찾는 중 (2/4)");
        assert!(t.is_open());
        assert_eq!(PetTask::decode(&obj(json!({"id": "t", "state": "waiting", "wait_reason": "approval"}))).status_text(), "승인을 기다려요");
        assert_eq!(
            PetTask::decode(&obj(json!({"id": "t", "state": "failed", "failure": {"code": "upstream_error"}}))).status_text(),
            ErrorText::user_message("upstream_error", None, "")
        );

        let mem = frame(r#"{"type":"memory_update","room_id":"r","update":{"update_id":"u1","action":"created","memory_ids":["m1"],"kind":"fact","preview":"파스타 좋아함","source":"chat","always_show":false,"items":[{"memory_id":"m1","kind":"fact","preview":"파스타 좋아함"}]}}"#);
        let ServerEvent::Memory(n) = mem.event else { panic!("memory") };
        let item = |id: &str, kind: &str, p: &str| PetMemoryItem { memory_id: id.into(), kind: kind.into(), preview: p.into() };
        assert_eq!(n.items, vec![item("m1", "fact", "파스타 좋아함")]);
        assert_eq!(n.text(), "기억했어요: 파스타 좋아함");
        // Without items[]: memory_ids + kind + preview.
        let old = PetMemoryNote::decode(&obj(json!({"update_id": "u2", "action": "created", "memory_ids": ["m2"], "kind": "episode", "preview": "여행"})));
        assert_eq!(old.items, vec![item("m2", "episode", "여행")]);

        let usage = frame(r#"{"type":"usage","room_id":"r","remaining":40,"available_to_start":30,"light_chat_used":3,"light_chat_cap":50,"state":{"plan_id":"free","allowance":100,"remaining":40,"period":"day","exhausted":false,"low_balance":true,"limits":[]}}"#);
        let ServerEvent::Usage(u) = usage.event else { panic!("usage") };
        assert_eq!(u.summary(), "크레딧 40/100 · 가벼운 대화 3/50 · 얼마 안 남았어요");

        let done = frame(r#"{"type":"done","room_id":"r","client_id":"c1","message_id":null,"task_id":"t1","stopped":false}"#);
        assert_eq!(done.task_id.as_deref(), Some("t1"));
        assert_eq!(
            frame(r#"{"type":"notification","notification":{"kind":"credit_granted","title":"크레딧","body":"10 받았어요"}}"#).event,
            ServerEvent::Notification { kind: "credit_granted".into(), title: "크레딧".into(), body: Some("10 받았어요".into()) }
        );
        assert_eq!(frame(r#"{"type":"compacted","room_id":"r"}"#).event, ServerEvent::Unknown { kind: "compacted".into() });
    }

    #[test]
    fn message_decode_keeps_files_memories_cards_and_flags() {
        let m = decode_msg(r#"{"id":"m9","seq":9,"role":"assistant","text":"정리했어요","content":{"type":"text","text":"정리했어요"},"used_memory_ids":["f1","f2"],"interim":false,"task_result":true,"task_id":"t1","attachments":[{"kind":"file","attachment_id":"a1","name":"표.csv","mime":"text/csv","bytes":10,"url":"https://x/s","origin":"agent"},{"kind":"image","deleted":true}],"attachment":{"file_id":"p1","name":"메모","kind":"page","deleted":false}}"#);
        assert_eq!(m.used_memory_ids, ["f1", "f2"]);
        assert!(m.task_result);
        assert_eq!(m.files.len(), 3);
        assert!(m.files[0].by_nacho);
        assert!(m.files[1].deleted);
        assert_eq!(m.files[2].kind, "page");
        let tool = ChatMessage::decode(&obj(json!({"id": "x", "role": "tool", "content": {"type": "tool_result"}})));
        assert!(!tool.is_visible());
        let note = ChatMessage::decode(&obj(json!({"id": "y", "role": "system", "text": "", "content": {"type": "system_note", "kind": "page_created", "text": "문서를 만들었어요"}})));
        assert_eq!(note.text, "문서를 만들었어요");
        assert!(note.is_visible());
    }

    // MARK: PetServerReducerTests

    fn connected() -> ChatState {
        let mut s = ChatState::new();
        s.apply(&frame(&format!(r#"{{"type":"ready","user_id":"u","agent_id":"a","agent_name":"나초","room_id":"{ROOM}"}}"#)));
        s
    }

    /// `{"type":"message","room_id":ROOM,<client>"message":<m>}`
    fn message_frame(m: &str, client: Option<&str>) -> ServerFrame {
        let c = client.map(|c| format!(r#""client_id":"{c}","#)).unwrap_or_default();
        frame(&format!(r#"{{"type":"message","room_id":"{ROOM}",{c}"message":{m}}}"#))
    }

    fn room_frame(rest: &str) -> ServerFrame { frame(&format!(r#"{{"room_id":"{ROOM}",{rest}}}"#)) }

    #[test]
    fn chat_turn_with_tools_interim_files_and_memories() {
        let mut s = connected();
        s.begin_turn("찾아줘", "c1", vec![]);
        s.apply(&room_frame(r#""type":"turn_started","client_id":"c1","turn_id":"t","duplicate":false"#));
        s.apply(&message_frame(&msg("u1", 11, "user", "찾아줘", Some("c1"), ""), Some("c1")));
        s.apply(&room_frame(r#""type":"tool_call_started","client_id":"c1","call_id":"k1","name":"search_web""#));
        s.apply(&message_frame(&msg("i1", 12, "assistant", "찾아보고 있어요", None, r#","interim":true"#), Some("c1")));
        s.apply(&room_frame(r#""type":"tool_call_finished","client_id":"c1","call_id":"k1","name":"search_web","meta":{"is_error":false}"#));
        s.apply(&room_frame(r#""type":"text_delta","client_id":"c1","text":"찾았""#));
        let answer = msg("a1", 13, "assistant", "찾았어요", None, r#","used_memory_ids":["f1"],"attachments":[{"kind":"file","name":"결과.md","origin":"agent","attachment_id":"x"}]"#)
            .replace(r#""used_memory_ids":[],"redactions":[],"attachments":[]"#, r#""redactions":[]"#);
        s.apply(&message_frame(&answer, Some("c1")));
        s.apply(&room_frame(r#""type":"memory_update","client_id":"c1","update":{"update_id":"u1","action":"created","items":[{"memory_id":"f9","kind":"fact","preview":"강남 좋아함"}]}"#));
        // The hub sends the same update again: one line.
        s.apply(&room_frame(r#""type":"memory_update","update":{"update_id":"u1","action":"created","items":[{"memory_id":"f9","kind":"fact","preview":"강남 좋아함"}]}"#));
        s.apply(&room_frame(r#""type":"done","client_id":"c1","message_id":"a1","stopped":false"#));

        let t = &s.turns[0];
        assert_eq!(t.status, TurnStatus::Done);
        assert_eq!(t.user_seq, Some(11));
        assert_eq!(t.answer_seq, Some(13));
        assert_eq!(t.assistant_text, "찾았어요");
        assert_eq!(t.progress, ["찾아보고 있어요"], "interim = progress line, not an answer");
        assert_eq!(t.tools, vec![ToolStep { call_id: "k1".into(), name: "search_web".into(), finished: true, is_error: false }]);
        assert_eq!(t.used_memory_ids, ["f1"]);
        assert_eq!(t.files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["결과.md"]);
        assert_eq!(s.memory_notes.len(), 1);
        assert_eq!(s.memory_notes[0].turn_client_id.as_deref(), Some("c1"));
        assert!(s.history.is_empty(), "nothing of this turn lands in the record");
        assert_eq!(s.last_seq(), 13);
    }

    #[test]
    fn turn_handed_to_a_task_gets_its_answer_later_from_the_hub() {
        let mut s = connected();
        s.begin_turn("예약해줘", "c2", vec![]);
        s.apply(&room_frame(r#""type":"turn_started","client_id":"c2","turn_id":"t","task_id":"tk","duplicate":false"#));
        s.apply(&room_frame(r#""type":"done","client_id":"c2","message_id":null,"task_id":"tk","stopped":false"#));
        assert_eq!(s.turns[0].task_id.as_deref(), Some("tk"));
        assert_eq!(s.turns[0].status, TurnStatus::Done);
        assert_eq!(s.turns[0].assistant_text, "");
        // Later, without client_id:
        s.apply(&room_frame(r#""type":"task_update","task":{"id":"tk","state":"running","title":"예약"}"#));
        assert_eq!(s.task.as_ref().map(|t| t.status_text()).as_deref(), Some("하고 있어요"));
        s.apply(&message_frame(&msg("i2", 21, "assistant", "자리를 보고 있어요", None, r#","interim":true,"task_id":"tk""#), None));
        s.apply(&room_frame(r#""type":"card_update","request":{"id":"cd","kind":"approval","status":"pending","version":1,"task_id":"tk","payload":{"title":"예약할까요?"}}"#));
        s.apply(&room_frame(r#""type":"card_update","request":{"id":"cd","kind":"approval","status":"answered","answered_action":"approve","version":2,"task_id":"tk","payload":{"title":"예약할까요?"}}"#));
        s.apply(&room_frame(r#""type":"card_update","request":{"id":"cd","kind":"approval","status":"pending","version":1,"payload":{"title":"old"}}"#));
        s.apply(&message_frame(&msg("r2", 23, "assistant", "7시에 예약했어요", None, r#","task_result":true,"task_id":"tk""#), None));
        s.apply(&room_frame(r#""type":"task_update","task":{"id":"tk","state":"done","title":"예약"}"#));
        assert_eq!(s.turns[0].progress, ["자리를 보고 있어요"]);
        assert_eq!(s.turns[0].assistant_text, "7시에 예약했어요");
        assert_eq!(s.cards.len(), 1);
        assert_eq!(s.cards[0].status, "answered", "an older version never overwrites a newer one");
        assert_eq!(s.cards[0].status_text(), "승인했어요");
        assert!(s.pending_cards().is_empty());
        assert_eq!(s.task.as_ref().map(|t| t.state.as_str()), Some("done"));
        assert!(s.history.is_empty());
        // Another device's message goes to the record.
        s.apply(&message_frame(&msg("o1", 30, "user", "폰에서 보냄", Some("phone-1"), ""), None));
        assert_eq!(s.history.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["폰에서 보냄"]);
    }

    #[test]
    fn history_merge_and_recovery_after_a_dropped_connection() {
        let mut s = connected();
        let page: Vec<ChatMessage> = [
            msg("h1", 1, "user", "어제 말", Some("x"), ""),
            msg("h2", 2, "assistant", "어제 답", None, ""),
            r#"{"id":"h3","seq":3,"role":"tool","content":{"type":"tool_result"}}"#.to_string(),
        ]
        .iter()
        .map(|m| decode_msg(m))
        .collect();
        s.merge_history(page.clone(), true, false);
        assert_eq!(s.history.iter().map(|m| m.seq).collect::<Vec<_>>(), [Some(1), Some(2)], "tool messages are not shown");
        assert!(s.has_more_history);
        assert!(s.history_loaded);
        s.merge_history(vec![ChatMessage::new("h0", Some(0), "user", "그제")], false, true);
        assert_eq!(s.history.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["h0", "h1", "h2"]);
        assert!(!s.has_more_history);
        s.merge_history(page, false, false); // same page again: no duplicates
        assert_eq!(s.history.len(), 3);

        s.begin_turn("오늘 날씨", "c3", vec![]);
        s.apply(&room_frame(r#""type":"turn_started","client_id":"c3","turn_id":"t","duplicate":false"#));
        s.apply(&room_frame(r#""type":"text_delta","client_id":"c3","text":"맑""#));
        s.connection_lost();
        assert_eq!(s.turns[0].status, TurnStatus::Interrupted);
        let delta: Vec<ChatMessage> = [
            msg("u3", 4, "user", "오늘 날씨", Some("c3"), ""),
            msg("i3", 5, "assistant", "보는 중", None, r#","interim":true"#),
            msg("a3", 6, "assistant", "맑아요", None, ""),
            msg("p1", 7, "user", "폰에서", Some("phone"), ""),
        ]
        .iter()
        .map(|m| decode_msg(m))
        .collect();
        s.recover(delta);
        assert_eq!(s.turns[0].status, TurnStatus::Done);
        assert_eq!(s.turns[0].assistant_text, "맑아요");
        assert_eq!(s.turns[0].progress, ["보는 중"]);
        assert_eq!(s.history.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["그제", "어제 말", "어제 답", "폰에서"]);
        assert_eq!(s.last_seq(), 7);
        // The echo of the question came before the drop: the delta holds only the answer.
        s.begin_turn("내일은?", "c4", vec![]);
        s.apply(&message_frame(&msg("u4", 8, "user", "내일은?", Some("c4"), ""), Some("c4")));
        s.connection_lost();
        s.recover(vec![decode_msg(&msg("a4", 9, "assistant", "비 와요", None, ""))]);
        assert_eq!(s.turns[1].status, TurnStatus::Done);
        assert_eq!(s.turns[1].assistant_text, "비 와요");
        assert_eq!(s.history.len(), 4);
        // A new room (other account): the record starts over.
        s.apply(&frame(r#"{"type":"ready","user_id":"u","agent_id":"a","agent_name":"나초","room_id":"other"}"#));
        assert!(s.history.is_empty());
        assert!(!s.history_loaded);
    }

    #[test]
    fn forget_echo_updates_the_line() {
        let mut s = connected();
        s.apply(&room_frame(r#""type":"memory_update","update":{"update_id":"u1","action":"created","items":[{"memory_id":"f1","kind":"fact","preview":"고양이"}]}"#));
        s.apply(&room_frame(r#""type":"memory_update","update":{"update_id":"u2","action":"forgotten","items":[{"memory_id":"f1","kind":"fact","preview":""}]}"#));
        assert_eq!(s.memory_notes.len(), 1);
        assert_eq!(s.memory_notes[0].action, "forgotten");
        s.apply(&room_frame(r#""type":"memory_update","update":{"update_id":"u3","action":"restored","items":[{"memory_id":"f1","kind":"fact","preview":"고양이"}]}"#));
        assert_eq!(s.memory_notes.len(), 1);
        assert_eq!(s.memory_notes[0].action, "created");
        assert_eq!(s.memory_notes[0].text(), "기억했어요: 고양이");
    }

    #[test]
    fn error_codes_from_the_server_list() {
        let codes = [
            "unauthorized", "validation_error", "invalid_input", "bad_frame", "credits_exhausted", "task_not_startable", "light_chat_limit",
            "quota_exceeded", "phone_verification_required", "forbidden", "suspended", "account_deleting", "not_member", "not_found", "conflict",
            "version_conflict", "expired", "superseded", "already_answered", "character_unavailable", "agent_unavailable", "connection_required",
            "connection_needs_reauth", "scope_blocked", "payload_too_large", "unsupported_media_type", "restore_expired", "too_many_requests",
            "internal_error", "not_implemented", "upstream_error", "transport", "outcome_unknown", "connector_unavailable", "service_disabled",
            "agent_limit", "payment_error", "estimate_changed", "vault_not_ready",
        ];
        for c in codes {
            let t = ErrorText::user_message(c, None, "english");
            assert!(!t.contains(&format!("({c})")), "{c} has its own text");
            assert!(!t.contains("english"));
        }
        assert_eq!(ErrorText::user_message("brand_new", None, "english"), "문제가 생겼어요. (brand_new)");
        assert_eq!(ErrorText::user_message("task_not_startable", Some("heavy_daily_limit"), ""), "지금은 일을 맡길 수 없어요. 오늘 맡길 수 있는 큰 일을 다 썼어요.");
        assert_eq!(ErrorText::user_message("character_unavailable", Some("deleting"), ""), "이 나초는 지워지는 중이에요.");
        assert!(ErrorText::user_message("too_many_requests", Some("2030-01-02T03:04:05Z"), "").contains("에 다시 해 주세요"));
        assert_eq!(ErrorText::user_message("too_many_requests", None, ""), "잠시 너무 많이 요청했어요. 잠시 뒤 다시 해 주세요.");
        assert_eq!(
            ErrorText::from_body(br#"{"error":{"code":"light_chat_limit","message":"x"}}"#, 402),
            ErrorText::user_message("light_chat_limit", None, "")
        );
        assert_eq!(ErrorText::from_body(b"<html>", 502), ErrorText::user_message("internal_error", None, ""));
        assert_eq!(ErrorText::from_body(b"<html>", 418), "문제가 생겼어요. (HTTP 418)");
    }

    // MARK: PetServerRequestTests

    #[test]
    fn history_delta_usage_config() {
        let e = ep("https://api.example.com");
        let h = PetServerAPI::history_request(&e, Some("tk"), "r1", Some(40), PetServerAPI::HISTORY_PAGE_SIZE);
        assert_eq!(h.method, "GET");
        assert_eq!(path(&h), "/api/v1/rooms/r1/history");
        let want: BTreeMap<String, String> = [("view", "app"), ("limit", "50"), ("before_seq", "40")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        assert_eq!(query(&h), want);
        assert_eq!(h.header("Authorization"), Some("Bearer tk"));
        assert!(!query(&PetServerAPI::history_request(&e, None, "r1", None, 50)).contains_key("before_seq"));
        let d = PetServerAPI::delta_request(&e, Some("tk"), "r1", 7);
        assert_eq!(path(&d), "/api/v1/rooms/r1/delta");
        assert_eq!(query(&d), [("view", "app"), ("since_seq", "7")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
        assert_eq!(path(&PetServerAPI::usage_request(&e, Some("tk"))), "/api/v1/me/usage");
        let c = PetServerAPI::client_config_request(&e);
        assert_eq!(path(&c), "/client/config", "not under /api/v1");
        assert_eq!(c.header("Authorization"), None);

        let page = PetServerAPI::decode_history(&PetHTTPResponse::new(200, format!("[{}]", msg("a", 1, "user", "hi", None, ""))).with_header("x-has-more", "true")).unwrap();
        assert_eq!(page.messages.first().map(|m| m.text.as_str()), Some("hi"));
        assert!(page.has_more);
        let delta = PetServerAPI::decode_delta(br#"{"room_id":"r","messages":[],"last_seq":9,"read_seq":8,"has_more":false,"activity":{"kind":"typing"}}"#).unwrap();
        assert_eq!(delta.last_seq, 9);
        assert_eq!(delta.activity, Some(ActivityKind::Typing));
        let usage = PetServerAPI::decode_usage(br#"{"plan_id":"free","allowance":100,"remaining":0,"available_to_start":0,"exhausted":true,"block_reason":"credits_exhausted","limits":[{"kind":"light_chat","used":50,"cap":50}]}"#);
        assert_eq!(usage.map(|u| u.summary()).as_deref(), Some("크레딧 0/100 · 가벼운 대화 50/50 · 크레딧이 모자라요."));
        let cfg = PetClientConfig::decode(r#"{"service_status":{"state":"maintenance","message":"3시까지"},"dictation":{"enabled":true,"max_sec":60,"max_bytes":2000000},"media_max_bytes":20971520}"#.as_bytes()).unwrap();
        assert!(cfg.dictation_enabled);
        assert_eq!(cfg.media_max_bytes, 20_971_520);
        assert_eq!(cfg.service_notice().as_deref(), Some("나초 서버 점검 중이에요. 3시까지"));
        assert_eq!(PetClientConfig::decode(b"{}").map(|c| c.dictation_enabled), Some(false));
    }

    #[test]
    fn dictation_multipart() {
        let e = ep("https://api.example.com");
        let r = PetServerAPI::dictation_request(&e, Some("tk"), &[0x41, 0x42], "audio/mp4", "d.m4a", "ko", Some(3.5), "BND");
        assert_eq!(r.method, "POST");
        assert_eq!(path(&r), "/api/v1/dictation");
        assert_eq!(r.header("Content-Type"), Some("multipart/form-data; boundary=BND"));
        assert_eq!(r.header("Authorization"), Some("Bearer tk"));
        let body = String::from_utf8(r.body.clone().unwrap()).unwrap();
        assert!(body.contains("--BND\r\nContent-Disposition: form-data; name=\"language\"\r\n\r\nko\r\n"));
        assert!(body.contains("name=\"duration_sec\"\r\n\r\n3.50\r\n"));
        assert!(body.contains("name=\"file\"; filename=\"d.m4a\"\r\nContent-Type: audio/mp4\r\n\r\nAB\r\n--BND--\r\n"));
        assert_eq!(PetServerAPI::decode_dictation(r#"{"text":"안녕","language":"ko"}"#.as_bytes()).as_deref(), Some("안녕"));
        let no_dur = PetServerAPI::dictation_request(&e, None, &[1], "audio/wav", "a\"b.wav", "ko", None, "B");
        let s = String::from_utf8_lossy(no_dur.body.as_deref().unwrap()).to_string();
        assert!(!s.contains("duration_sec") && s.contains("filename=\"ab.wav\""));
        assert_eq!(no_dur.header("Authorization"), None);
    }

    #[test]
    fn memory_and_card_requests() {
        let e = ep("https://api.example.com");
        let f = PetServerAPI::feedback_request(&e, Some("tk"), "틀렸어", "correction", &["f1".into()], Some("r1"), Some("a1"), "k1");
        assert_eq!(path(&f), "/api/v1/memory/feedback");
        assert_eq!(f.header("Idempotency-Key"), Some("k1"));
        let fj = body(&f);
        assert_eq!(fj.str("text").as_deref(), Some("틀렸어"));
        assert_eq!(fj.str("kind").as_deref(), Some("correction"));
        assert_eq!(fj.strs("memory_ids"), ["f1"]);
        assert_eq!(fj.str("room_id").as_deref(), Some("r1"));
        let forget_fact = PetServerAPI::forget_request(&e, Some("tk"), "f1", "fact");
        assert_eq!(forget_fact.method, "DELETE");
        assert_eq!(path(&forget_fact), "/api/v1/memory/facts/f1");
        assert_eq!(path(&PetServerAPI::forget_request(&e, Some("tk"), "e1", "episode")), "/api/v1/memory/episodes/e1");
        let restore = PetServerAPI::restore_request(&e, Some("tk"), "f1");
        assert_eq!(restore.method, "POST");
        assert_eq!(path(&restore), "/api/v1/memory/f1/restore");

        let mut card = PetCard::decode(&obj(json!({"id": "cd", "kind": "choice", "status": "pending", "version": 3, "payload": {"question": "q"}})));
        let a = PetServerAPI::answer_card_request(&e, Some("tk"), &card, "choose", &["b".into()], None);
        assert_eq!(path(&a), "/api/v1/chat/requests/cd/answer");
        let aj = body(&a);
        assert_eq!(aj.str("action").as_deref(), Some("choose"));
        assert_eq!(aj.str("option_id").as_deref(), Some("b"));
        assert_eq!(aj.int("expected_version"), Some(3));
        card.multiple = true;
        assert_eq!(body(&PetServerAPI::answer_card_request(&e, Some("tk"), &card, "choose", &["a".into(), "b".into()], None)).strs("option_ids"), ["a", "b"]);
    }

    /// From PetVoiceChoiceTests.swift: an approval with rule options.
    #[test]
    fn answer_request_carries_allow_rule_only_with_approve() {
        let e = ep("https://api.example.com");
        let c = PetCard::decode(&obj(json!({"id": "ra", "kind": "approval", "status": "pending", "version": 1, "task_id": "t1",
            "payload": {"title": "배달앱 12,000원 결제할까요?", "rule_options": [{"id": "pay_day", "summary": "하루 30,000원까지"}, {"id": "pay_week", "summary": "일주일 60,000원"}]}})));
        let j = body(&PetServerAPI::answer_card_request(&e, Some("t"), &c, "approve", &[], Some("pay_week")));
        assert_eq!(j.str("action").as_deref(), Some("approve"));
        assert_eq!(j.obj("allow_rule").and_then(|r| r.str("rule_option_id")).as_deref(), Some("pay_week"));
        assert_eq!(j.int("expected_version"), Some(1));
        assert!(body(&PetServerAPI::answer_card_request(&e, Some("t"), &c, "approve", &[], None)).get("allow_rule").is_none(), "이번만 실행: no rule");
        assert!(body(&PetServerAPI::answer_card_request(&e, Some("t"), &c, "deny", &[], Some("pay_week"))).get("allow_rule").is_none());
    }

    // MARK: PetTaskStartModelTests (PetTaskStartTests.swift)

    const TASK_ROOM: &str = "0193aaaa-0000-7000-8000-0000000000r7";
    const TASK_ID: &str = "0193aaaa-0000-7000-8000-00000000t001";

    fn task_json(state: &str, version: i64) -> String {
        format!(
            r#"{{"id":"{TASK_ID}","room_id":"{TASK_ROOM}","kind":"task","state":"{state}","title":"휴대폰 전화해줘","request_text":"x","plan_mode":"full","steps":[],"stalled":false,"credit":{{"estimate":{{"estimate_min":40,"estimate_max":60,"task_label":"전화"}},"estimate_id":"e1","used":0,"cap":60,"refund_status":"none"}},"accessed_items":[],"plan_blocks":[],"result_blocks":[],"estimate_id":"e1","version":{version}}}"#
        )
    }

    #[test]
    fn planned_task_decodes_its_estimate_and_asks_for_confirmation() {
        let ServerEvent::Task(t) = frame(&format!(r#"{{"type":"task_update","room_id":"{TASK_ROOM}","task":{}}}"#, task_json("planned", 1))).event else {
            panic!("task_update")
        };
        assert_eq!(t.estimate_id.as_deref(), Some("e1"));
        assert_eq!(t.estimate_text().as_deref(), Some("약 40~60크레딧"));
        assert_eq!(t.version, 1);
        assert!(t.needs_start_confirm());
        assert_eq!(t.status_text(), "시작 확인 필요", "planned no longer reads 곧 시작해요");
        assert_eq!(PetTask::new("q", "queued", "x").status_text(), "곧 시작해요");
        // The top-level estimate_id is what the server compares; credit.estimate_id is the fallback.
        let only_credit = PetTask::decode_data(br#"{"id":"a","state":"planned","title":"t","credit":{"estimate_id":"c9","used":0,"refund_status":"none"}}"#).unwrap();
        assert_eq!(only_credit.estimate_id.as_deref(), Some("c9"));
        assert_eq!(only_credit.estimate_text(), None);
    }

    #[test]
    fn requests_follow_the_contract() {
        let e = ep("https://api.example");
        let start = PetServerAPI::start_task_request(&e, Some("tk"), TASK_ID, Some("e1"));
        assert_eq!(start.method, "POST");
        assert_eq!(path(&start), format!("/api/v1/tasks/{TASK_ID}/start"));
        assert_eq!(start.header("Authorization"), Some("Bearer tk"));
        assert_eq!(body(&start).str("expected_estimate_id").as_deref(), Some("e1"));
        assert!(body(&PetServerAPI::start_task_request(&e, Some("tk"), TASK_ID, None)).is_empty());
        let cancel = PetServerAPI::cancel_task_request(&e, Some("tk"), TASK_ID);
        assert_eq!(cancel.method, "POST");
        assert_eq!(path(&cancel), format!("/api/v1/tasks/{TASK_ID}/cancel"));
        assert_eq!(path(&PetServerAPI::task_request(&e, Some("tk"), TASK_ID)), format!("/api/v1/tasks/{TASK_ID}"));
    }

    #[test]
    fn an_older_task_version_never_overwrites_a_newer_one() {
        let mut s = ChatState::new();
        s.apply_task(PetTask::decode_data(task_json("running", 3).as_bytes()).unwrap(), true);
        s.apply_task(PetTask::decode_data(task_json("planned", 1).as_bytes()).unwrap(), true);
        assert_eq!(s.tasks.get(TASK_ID).map(|t| t.state.as_str()), Some("running"));
        assert_eq!(s.task.as_ref().map(|t| t.state.as_str()), Some("running"));
    }

    // MARK: PetServerContractTests.swift

    /// "GET /api/v1/rooms/{id}/history" for a built request (ids → {id}, query dropped).
    fn route(r: &PetHTTPRequest, ids: &[&str]) -> String {
        let mut p = path(r);
        for id in ids { p = p.replace(&format!("/{id}"), "/{id}"); }
        format!("{} {p}", r.method)
    }

    #[test]
    fn every_request_the_pet_builds_is_in_the_server_spec() {
        let e = ep("https://api.nachotown.com");
        let room = "0193aaaa-0000-7000-8000-000000000003";
        let mem = "0193aaaa-0000-7000-8000-0000000000f1";
        let card = PetCard::decode(&obj(json!({"id": "0193aaaa-0000-7000-8000-0000000000c1", "kind": "choice", "version": 1})));
        let agent = "0193aaaa-0000-7000-8000-000000000002";
        let task = "0193aaaa-0000-7000-8000-0000000000a1";
        let file = PetMedia::prepare("a.png", vec![0x89, 0x50, 0x4e, 0x47], None).unwrap();
        let t = Some("t");
        let built = [
            PetServerAPI::client_config_request(&e),
            PetHTTPRequest::get(e.rest("/api/v1/me"), t),
            PetServerAPI::usage_request(&e, t),
            PetHTTPRequest::get(e.rest("/api/v1/connections"), t), // connectors module (not ported here yet)
            PetHTTPRequest::get(e.rest("/api/v1/tools"), t),
            PetMedia::upload_request(&e, t, &file),
            // ConnectorBinding.request lives in the connector module (another port): its route, built by hand.
            PetHTTPRequest::post_json(e.rest(&format!("/api/v1/agents/{agent}/connector-binding")), t, &json!({})),
            PetServerAPI::history_request(&e, t, room, Some(5), 50),
            PetServerAPI::delta_request(&e, t, room, 5),
            PetServerAPI::dictation_request(&e, t, &[1], "audio/mp4", "d.m4a", "ko", Some(1.0), "b"),
            PetServerAPI::feedback_request(&e, t, "x", "correction", &[mem.into()], Some(room), None, "k"),
            PetServerAPI::forget_request(&e, t, mem, "fact"),
            PetServerAPI::forget_request(&e, t, mem, "episode"),
            PetServerAPI::restore_request(&e, t, mem),
            PetServerAPI::card_request(&e, t, &card.id),
            PetServerAPI::answer_card_request(&e, t, &card, "choose", &["a".into()], None),
            PetServerAPI::task_request(&e, t, task),
            PetServerAPI::start_task_request(&e, t, task, Some("e")),
            PetServerAPI::cancel_task_request(&e, t, task),
        ];
        let used: BTreeSet<String> = built.iter().map(|r| route(r, &[room, mem, &card.id, agent, task])).collect();
        let listed: BTreeSet<String> = PET_SERVER_ROUTES.iter().map(|s| s.to_string()).collect();
        assert_eq!(listed.len(), PET_SERVER_ROUTES.len(), "no route listed twice");
        assert_eq!(used.difference(&listed).collect::<Vec<_>>(), Vec::<&String>::new(), "not in {PET_SERVER_SPEC_REF}");
        assert_eq!(listed.difference(&used).collect::<Vec<_>>(), Vec::<&String>::new(), "listed but never built");
    }

    #[test]
    fn route_list_is_one_route_per_line_for_the_contract_script() {
        let src = include_str!("server_api.rs");
        let start = src.find("pub const PET_SERVER_ROUTES").unwrap();
        let block = &src[start..start + src[start..].find("];").unwrap()];
        let lines: Vec<&str> = block.lines().skip(1).collect();
        assert_eq!(lines.len(), PET_SERVER_ROUTES.len());
        for (l, r) in lines.iter().zip(PET_SERVER_ROUTES) { assert_eq!(*l, format!("    \"{r}\","), "the sed pattern reads this line"); }
    }

    // Rust-only: PetServerCalls against the fake network (the Swift checks these through the session tests).

    fn calls(http: &Arc<FakeHTTP>) -> PetServerCalls { PetServerCalls::new(ep("https://api.example.com"), http.clone(), Some("tk".into())) }

    type Slot<T> = Arc<Mutex<Option<Result<T, PetAPIError>>>>;

    fn capture<T: Send + 'static>() -> (Slot<T>, APICompletion<T>) {
        let slot = Arc::new(Mutex::new(None));
        let s2 = slot.clone();
        (slot, Box::new(move |r| *s2.lock().unwrap() = Some(r)))
    }

    #[test]
    fn calls_map_errors_and_follow_delta_pages() {
        let http = FakeHTTP::new();
        let c = calls(&http);
        http.reply(Err(PetNetError("down".into())));
        let (got, done) = capture();
        c.usage(done);
        assert_eq!(got.lock().unwrap().take(), Some(Err(PetAPIError::network())));
        http.reply_json(401, r#"{"error":{"code":"unauthorized","message":"x"}}"#);
        let (got, done) = capture::<PetUsage>();
        c.usage(done);
        let e = got.lock().unwrap().take().unwrap().unwrap_err();
        assert!(e.is_unauthorized());
        assert_eq!(e.text, "로그인이 필요해요.");
        http.reply_json(200, "[]");
        let (got, done) = capture::<PetUsage>();
        c.usage(done);
        assert_eq!(got.lock().unwrap().take(), Some(Err(PetAPIError::bad_response())));

        let page = |seqs: &[i64], last: i64, more: bool| {
            let m: Vec<String> = seqs.iter().map(|s| msg(&format!("m{s}"), *s, "assistant", "x", None, "")).collect();
            format!(r#"{{"messages":[{}],"last_seq":{last},"read_seq":0,"has_more":{more}}}"#, m.join(","))
        };
        http.reply_json(200, &page(&[11, 12], 20, true));
        http.reply_json(200, &page(&[13], 20, false));
        let (got, done) = capture();
        c.delta(ROOM, 10, done);
        let d = got.lock().unwrap().take().unwrap().unwrap();
        assert_eq!(d.messages.iter().filter_map(|m| m.seq).collect::<Vec<_>>(), [11, 12, 13]);
        let sent = http.sent();
        assert_eq!(query(&sent[sent.len() - 2]).get("since_seq").map(String::as_str), Some("10"));
        assert_eq!(query(&sent[sent.len() - 1]).get("since_seq").map(String::as_str), Some("12"));
    }

    #[test]
    fn start_task_estimate_changed_is_an_answer() {
        let http = FakeHTTP::new();
        let c = calls(&http);
        http.reply_json(200, &task_json("running", 2));
        let (got, done) = capture();
        c.start_task(TASK_ID, Some("e1"), done);
        assert!(matches!(got.lock().unwrap().take(), Some(Ok(PetTaskStart::Started(t))) if t.state == "running"));
        http.reply_json(409, &format!(r#"{{"error":{{"code":"estimate_changed","message":"x"}},"task":{}}}"#, task_json("planned", 3)));
        let (got, done) = capture();
        c.start_task(TASK_ID, Some("e1"), done);
        assert!(matches!(got.lock().unwrap().take(), Some(Ok(PetTaskStart::EstimateChanged(t))) if t.version == 3));
        http.reply_json(409, r#"{"error":{"code":"conflict","message":"x"}}"#);
        let (got, done) = capture::<PetTaskStart>();
        c.start_task(TASK_ID, None, done);
        assert_eq!(got.lock().unwrap().take().unwrap().unwrap_err().code.as_deref(), Some("conflict"));
    }
}

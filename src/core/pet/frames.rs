//! `/ws/chat` frames (nacho-server `docs/ws-protocol.md` §1). Unknown `type`s and fields are ignored. Port of
//! `PetFrames.swift`.
use super::json::{self, JsonExt};
use super::server_models::{ChatMessage, PetCard, PetMemoryNote, PetTask, PetUsage};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityKind { Thinking, Typing, Tool, Retrying, Idle }

impl ActivityKind {
    pub fn from_raw(s: &str) -> Option<Self> {
        Some(match s {
            "thinking" => Self::Thinking,
            "typing" => Self::Typing,
            "tool" => Self::Tool,
            "retrying" => Self::Retrying,
            "idle" => Self::Idle,
            _ => return None,
        })
    }
    pub fn raw(self) -> &'static str {
        match self {
            Self::Thinking => "thinking",
            Self::Typing => "typing",
            Self::Tool => "tool",
            Self::Retrying => "retrying",
            Self::Idle => "idle",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ReadyInfo {
    pub user_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub room_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ServerEvent {
    Ready(ReadyInfo),
    TurnStarted { turn_id: String, duplicate: bool },
    TextDelta(String),
    Activity { kind: ActivityKind, tool: Option<String> },
    Message(ChatMessage),
    Usage(PetUsage),
    ToolStarted { call_id: String, name: String },
    ToolFinished { call_id: String, name: String, is_error: bool, duration_ms: Option<i64> },
    Card(PetCard),
    Task(PetTask),
    Memory(PetMemoryNote),
    Notification { kind: String, title: String, body: Option<String> },
    Done { message_id: Option<String>, stopped: bool },
    Error(ErrorInfo),
    Pong,
    Unknown { kind: String },
}

/// One decoded server frame. `client_id` is present on events of a turn started by this session.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerFrame {
    pub room_id: Option<String>,
    pub client_id: Option<String>,
    pub event: ServerEvent,
    /// `task_id` of `turn_started` / `done` (the turn was handed to a task) and of task-scoped events.
    pub task_id: Option<String>,
}

impl ServerFrame {
    pub fn new(room_id: Option<&str>, client_id: Option<&str>, event: ServerEvent) -> Self {
        Self { room_id: room_id.map(Into::into), client_id: client_id.map(Into::into), event, task_id: None }
    }

    /// None when `text` is not a JSON object with a string `type`.
    pub fn decode(text: &str) -> Option<ServerFrame> {
        let obj = json::object_str(text)?;
        let kind = obj.str("type")?;
        let s = |k: &str| obj.str(k);
        let unknown = || ServerEvent::Unknown { kind: kind.clone() };
        let event = match kind.as_str() {
            "ready" => ServerEvent::Ready(ReadyInfo {
                user_id: s("user_id").unwrap_or_default(),
                agent_id: s("agent_id").unwrap_or_default(),
                agent_name: s("agent_name").unwrap_or_default(),
                room_id: s("room_id").unwrap_or_default(),
            }),
            "turn_started" => ServerEvent::TurnStarted { turn_id: s("turn_id").unwrap_or_default(), duplicate: obj.bool("duplicate").unwrap_or(false) },
            "text_delta" => ServerEvent::TextDelta(s("text").unwrap_or_default()),
            "activity" => {
                let act = obj.obj("activity");
                let k = act.as_ref().and_then(|a| a.str("kind")).and_then(|k| ActivityKind::from_raw(&k)).unwrap_or(ActivityKind::Idle);
                ServerEvent::Activity { kind: k, tool: act.and_then(|a| a.str("tool")) }
            }
            "message" => match obj.obj("message") { Some(m) => ServerEvent::Message(ChatMessage::decode(&m)), None => unknown() },
            "usage" => PetUsage::decode(&obj).map(ServerEvent::Usage).unwrap_or_else(unknown),
            "tool_call_started" => ServerEvent::ToolStarted { call_id: s("call_id").unwrap_or_default(), name: s("name").unwrap_or_default() },
            "tool_call_finished" => {
                let meta = obj.obj("meta").unwrap_or_default();
                ServerEvent::ToolFinished {
                    call_id: s("call_id").unwrap_or_default(),
                    name: s("name").unwrap_or_default(),
                    is_error: meta.bool("is_error").unwrap_or(false),
                    duration_ms: meta.int("duration_ms"),
                }
            }
            "card_update" => obj.obj("request").map(|r| ServerEvent::Card(PetCard::decode(&r))).unwrap_or_else(unknown),
            "task_update" => obj.obj("task").map(|t| ServerEvent::Task(PetTask::decode(&t))).unwrap_or_else(unknown),
            "memory_update" => obj.obj("update").map(|u| ServerEvent::Memory(PetMemoryNote::decode(&u))).unwrap_or_else(unknown),
            "notification" => {
                let n = obj.obj("notification").unwrap_or_default();
                ServerEvent::Notification { kind: n.str("kind").unwrap_or_default(), title: n.str("title").unwrap_or_default(), body: n.str("body") }
            }
            "done" => ServerEvent::Done { message_id: s("message_id"), stopped: obj.bool("stopped").unwrap_or(false) },
            "error" => ServerEvent::Error(ErrorInfo {
                code: s("code").unwrap_or_else(|| "unknown".into()),
                message: s("message").unwrap_or_default(),
                reason: s("reason"),
            }),
            "pong" => ServerEvent::Pong,
            _ => unknown(),
        };
        Some(ServerFrame { room_id: s("room_id"), client_id: s("client_id"), event, task_id: s("task_id") })
    }
}

/// Frames the app sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientFrame {
    /// `attachment_ids` are ids returned by `POST /api/v1/media/uploads` (sent as `attachments: [{attachment_id}]`).
    Message { text: String, room_id: Option<String>, client_msg_id: String, attachment_ids: Vec<String> },
    Stop { room_id: String },
    Read { room_id: String, seq: i64 },
    Ping,
}

impl ClientFrame {
    pub fn message(text: &str, room_id: Option<&str>, client_msg_id: &str) -> Self {
        Self::Message { text: text.into(), room_id: room_id.map(Into::into), client_msg_id: client_msg_id.into(), attachment_ids: vec![] }
    }

    /// Compact JSON, keys sorted.
    pub fn encode(&self) -> String {
        let v: Value = match self {
            Self::Message { text, room_id, client_msg_id, attachment_ids } => {
                let mut o = json!({"type": "message", "text": text, "client_msg_id": client_msg_id, "client_id": client_msg_id});
                if let Some(r) = room_id { o["room_id"] = json!(r); }
                if !attachment_ids.is_empty() {
                    o["attachments"] = Value::Array(attachment_ids.iter().map(|a| json!({"attachment_id": a})).collect());
                }
                o
            }
            Self::Stop { room_id } => json!({"type": "stop", "room_id": room_id}),
            Self::Read { room_id, seq } => json!({"type": "read", "room_id": room_id, "seq": seq}),
            Self::Ping => json!({"type": "ping"}),
        };
        json::encode(&v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_turn_frames() {
        let f = ServerFrame::decode(r#"{"type":"ready","user_id":"u","agent_id":"a","agent_name":"나초","room_id":"r"}"#).unwrap();
        assert_eq!(f.event, ServerEvent::Ready(ReadyInfo { user_id: "u".into(), agent_id: "a".into(), agent_name: "나초".into(), room_id: "r".into() }));
        let f = ServerFrame::decode(r#"{"type":"turn_started","client_id":"c","turn_id":"t","task_id":"k"}"#).unwrap();
        assert_eq!(f.client_id.as_deref(), Some("c"));
        assert_eq!(f.task_id.as_deref(), Some("k"));
        assert_eq!(f.event, ServerEvent::TurnStarted { turn_id: "t".into(), duplicate: false });
        let f = ServerFrame::decode(r#"{"type":"activity","activity":{"kind":"tool","tool":"search_web"}}"#).unwrap();
        assert_eq!(f.event, ServerEvent::Activity { kind: ActivityKind::Tool, tool: Some("search_web".into()) });
        let f = ServerFrame::decode(r#"{"type":"activity","activity":{"kind":"dancing"}}"#).unwrap();
        assert_eq!(f.event, ServerEvent::Activity { kind: ActivityKind::Idle, tool: None });
        let f = ServerFrame::decode(r#"{"type":"tool_call_finished","call_id":"1","name":"x","meta":{"is_error":true,"duration_ms":12}}"#).unwrap();
        assert_eq!(f.event, ServerEvent::ToolFinished { call_id: "1".into(), name: "x".into(), is_error: true, duration_ms: Some(12) });
        let f = ServerFrame::decode(r#"{"type":"error","reason":"r"}"#).unwrap();
        assert_eq!(f.event, ServerEvent::Error(ErrorInfo { code: "unknown".into(), message: "".into(), reason: Some("r".into()) }));
        assert_eq!(ServerFrame::decode(r#"{"type":"message"}"#).unwrap().event, ServerEvent::Unknown { kind: "message".into() });
        assert_eq!(ServerFrame::decode(r#"{"type":"brand_new"}"#).unwrap().event, ServerEvent::Unknown { kind: "brand_new".into() });
        assert!(ServerFrame::decode(r#"{"no":"type"}"#).is_none());
        assert!(ServerFrame::decode("not json").is_none());
    }

    #[test]
    fn message_frame_carries_attachment_ids() {
        let f = ClientFrame::Message { text: "봐".into(), room_id: Some("r".into()), client_msg_id: "c1".into(), attachment_ids: vec!["a1".into(), "a2".into()] };
        assert_eq!(f.encode(), r#"{"attachments":[{"attachment_id":"a1"},{"attachment_id":"a2"}],"client_id":"c1","client_msg_id":"c1","room_id":"r","text":"봐","type":"message"}"#);
        assert_eq!(ClientFrame::message("hi", None, "c").encode(), r#"{"client_id":"c","client_msg_id":"c","text":"hi","type":"message"}"#);
        assert_eq!(ClientFrame::Read { room_id: "r".into(), seq: 7 }.encode(), r#"{"room_id":"r","seq":7,"type":"read"}"#);
        assert_eq!(ClientFrame::Ping.encode(), r#"{"type":"ping"}"#);
    }
}

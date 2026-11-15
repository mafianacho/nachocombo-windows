//! Which Nacho picture the pet shows (the original Nacho art, all rights reserved). Mapping decided by the owners:
//! idle = idle, listening = calling, thinking = thinking, answering = working, finished = happy, a long wait = tired,
//! then eyes-closed. `Waiting` is used while connecting. Port of `PetFace.swift`.
use super::chat_state::{ChatState, ConnectionPhase};
use super::frames::ActivityKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PetFace { Idle, Listening, Thinking, Speaking, Happy, Waiting, Tired, Sleeping }

impl PetFace {
    pub const ALL: [PetFace; 8] =
        [Self::Idle, Self::Listening, Self::Thinking, Self::Speaking, Self::Happy, Self::Waiting, Self::Tired, Self::Sleeping];

    /// Seconds the happy face stays after an answer finished.
    pub const HAPPY_FOR: f64 = 4.0;
    pub const TIRED_AFTER: f64 = 10.0 * 60.0;
    pub const SLEEP_AFTER: f64 = 30.0 * 60.0;

    /// The Swift raw value ("idle", "listening", …), also used by the motion fixture.
    pub fn raw(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Listening => "listening",
            Self::Thinking => "thinking",
            Self::Speaking => "speaking",
            Self::Happy => "happy",
            Self::Waiting => "waiting",
            Self::Tired => "tired",
            Self::Sleeping => "sleeping",
        }
    }

    pub fn from_raw(s: &str) -> Option<Self> { Self::ALL.into_iter().find(|f| f.raw() == s) }

    /// Picture name (without extension) in the bundled Nacho art.
    pub fn asset_name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Listening => "calling",
            Self::Thinking => "thinking",
            Self::Speaking => "working",
            Self::Happy => "happy",
            Self::Waiting => "waiting",
            Self::Tired => "tired",
            Self::Sleeping => "eyes-closed",
        }
    }

    pub fn korean_label(self) -> &'static str {
        match self {
            Self::Idle => "대기",
            Self::Listening => "듣는 중",
            Self::Thinking => "생각 중",
            Self::Speaking => "답변 중",
            Self::Happy => "다 했어요",
            Self::Waiting => "연결 중",
            Self::Tired => "심심해요",
            Self::Sleeping => "조는 중",
        }
    }

    /// `now` and `last_interaction` (the last click, drop, message or answer) in Unix seconds.
    pub fn derive(dictating: bool, chat: &ChatState, now: f64, last_interaction: f64) -> PetFace {
        if dictating { return Self::Listening; }
        if let Some(i) = chat.active_turn_index() {
            let t = &chat.turns[i];
            if chat.activity == ActivityKind::Typing || !t.assistant_text.is_empty() { return Self::Speaking; }
            return Self::Thinking;
        }
        match chat.activity {
            ActivityKind::Thinking | ActivityKind::Tool | ActivityKind::Retrying => return Self::Thinking,
            ActivityKind::Typing => return Self::Speaking,
            ActivityKind::Idle => {}
        }
        if let Some(done) = chat.last_finished_at {
            let since = now - done;
            if (0.0..Self::HAPPY_FOR).contains(&since) { return Self::Happy; }
        }
        if matches!(chat.phase, ConnectionPhase::Connecting | ConnectionPhase::Reconnecting { .. }) { return Self::Waiting; }
        let quiet = now - last_interaction;
        if quiet >= Self::SLEEP_AFTER { return Self::Sleeping; }
        if quiet >= Self::TIRED_AFTER { return Self::Tired; }
        Self::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::pet::frames::ServerFrame;
    use std::collections::HashSet;

    fn frame(json: &str) -> ServerFrame { ServerFrame::decode(json).expect("frame") }

    fn connected() -> ChatState {
        let mut s = ChatState::new();
        s.apply(&frame(r#"{"type":"ready","room_id":"r","agent_id":"a","agent_name":"나초"}"#));
        s
    }

    // PetCoreTests.swift / PetFaceTests
    #[test]
    fn long_wait_and_connecting_faces() {
        let t0 = 0.0;
        let mut chat = ChatState::new();
        assert_eq!(PetFace::derive(false, &chat, t0 + 60.0, t0), PetFace::Idle);
        assert_eq!(PetFace::derive(false, &chat, t0 + PetFace::TIRED_AFTER, t0), PetFace::Tired);
        assert_eq!(PetFace::derive(false, &chat, t0 + PetFace::SLEEP_AFTER, t0), PetFace::Sleeping);
        chat.phase = ConnectionPhase::Reconnecting { attempt: 1 };
        assert_eq!(PetFace::derive(false, &chat, t0, t0), PetFace::Waiting);
        assert_eq!(PetFace::derive(true, &chat, t0, t0), PetFace::Listening);
        let names: HashSet<_> = PetFace::ALL.iter().map(|f| f.asset_name()).collect();
        assert_eq!(names.len(), PetFace::ALL.len());
    }

    // PetChatTests.swift: the face parts of testPetStates and testStreamingTurnHappyPath (their chat parts are
    // covered in chat_state.rs).
    #[test]
    fn pet_states() {
        let mut s = connected();
        assert_eq!(PetFace::derive(true, &s, 0.0, 0.0), PetFace::Listening);
        s.begin_turn("q", "c-1", vec![]);
        assert_eq!(PetFace::derive(false, &s, 0.0, 0.0), PetFace::Thinking);
        s.apply(&frame(r#"{"type":"activity","activity":{"kind":"tool","tool":"search_web"}}"#));
        assert_eq!(s.active_tool.as_deref(), Some("search_web"));
        assert_eq!(PetFace::derive(false, &s, 0.0, 0.0), PetFace::Thinking);
        assert_eq!(PetFace::ALL.len(), 8);
        assert_eq!(PetFace::from_raw("speaking"), Some(PetFace::Speaking));
    }

    #[test]
    fn streaming_turn_faces() {
        let mut s = connected();
        s.begin_turn("안녕", "c-1", vec![]);
        s.apply(&frame(r#"{"type":"turn_started","room_id":"r","turn_id":"t","duplicate":false,"client_id":"c-1"}"#));
        s.apply(&frame(r#"{"type":"text_delta","room_id":"r","text":"반가","client_id":"c-1"}"#));
        assert_eq!(PetFace::derive(false, &s, 100.0, 100.0), PetFace::Speaking);
        s.apply(&frame(r#"{"type":"done","room_id":"r","stopped":false,"client_id":"c-1"}"#));
        let done = s.last_finished_at.expect("finished time");
        assert_eq!(PetFace::derive(false, &s, done, done), PetFace::Happy);
        assert_eq!(PetFace::derive(false, &s, done + 5.0, done), PetFace::Idle);
    }
}

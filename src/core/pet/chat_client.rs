//! `/ws/chat` client pieces. Port of `PetChatClient.swift`, part 1: the transport contract every WebSocket
//! implementation follows (WinHTTP on Windows, a plain socket in the dev CLI, a fake in tests), the outgoing text rules
//! and the send refusals. The client itself (connect, stream, stop, ping, reconnect) builds on these.
//!
//! Threading model of the pet core (same for every client and the session): state lives behind `Arc<Mutex<…>>`;
//! transport handlers, HTTP completions and scheduled timers may run on any thread; a `generation` counter drops the
//! callbacks of an older connection; `on_change` is called after every change, never while a lock is held.
use super::media::PetMedia;
use super::reconnect::TransportFailure;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportEvent {
    Opened,
    Text(String),
    /// None = clean close.
    Closed(Option<TransportFailure>),
}

pub type TransportHandler = Arc<dyn Fn(TransportEvent) + Send + Sync>;

/// One WebSocket connection. A new transport object is made for every connection attempt.
pub trait WebSocketTransport: Send + Sync {
    /// `url` is `ws://` or `wss://`; `bearer` goes in `Authorization: Bearer …` of the upgrade request.
    fn connect(&self, url: &str, bearer: Option<&str>, handler: TransportHandler);
    fn send(&self, text: &str);
    fn close(&self);
}

pub type MakeTransport = Arc<dyn Fn() -> Box<dyn WebSocketTransport> + Send + Sync>;

/// Runs `work` after `delay` seconds (on any thread). Tests pass a manual scheduler.
pub type Schedule = Arc<dyn Fn(f64, Box<dyn FnOnce() + Send>) + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutgoingProblem { Empty, TooLong }

impl OutgoingProblem {
    pub fn korean_message(&self) -> String {
        match self {
            Self::Empty => "보낼 내용을 입력해 주세요.".into(),
            Self::TooLong => format!("한 번에 보낼 수 있는 길이는 {}자까지예요.", OutgoingText::MAX_CODE_POINTS),
        }
    }
}

pub struct OutgoingText;

impl OutgoingText {
    /// Client-side limit (the server's limit is not documented).
    pub const MAX_CODE_POINTS: usize = 4000;

    /// Trims, rejects empty / over-long text, drops NUL. Length is counted in Unicode code points. `allow_empty`: a
    /// message that carries attachments may have no text (the server accepts attachment-only turns).
    pub fn validate(raw: &str, allow_empty: bool) -> Result<String, OutgoingProblem> {
        let t = raw.replace('\0', "");
        let t = t.trim();
        if t.is_empty() && !allow_empty { return Err(OutgoingProblem::Empty); }
        if t.chars().count() > Self::MAX_CODE_POINTS { return Err(OutgoingProblem::TooLong); }
        Ok(t.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendRefusal { InvalidText(OutgoingProblem), NotConnected, Busy, TooManyAttachments }

impl SendRefusal {
    pub fn korean_message(&self) -> String {
        match self {
            Self::InvalidText(p) => p.korean_message(),
            Self::TooManyAttachments => format!("한 번에 붙일 수 있는 파일은 {}개까지예요.", PetMedia::MAX_ATTACHMENTS),
            Self::NotConnected => "서버에 연결되어 있지 않아요.".into(),
            Self::Busy => "나초가 아직 답하고 있어요. 끝난 뒤에 보내 주세요.".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outgoing_text_rules() {
        assert_eq!(OutgoingText::validate("  안녕\0 \n", false), Ok("안녕".into()));
        assert_eq!(OutgoingText::validate("  ", false), Err(OutgoingProblem::Empty));
        assert_eq!(OutgoingText::validate("  ", true), Ok("".into()));
        assert_eq!(OutgoingText::validate(&"가".repeat(4000), false).map(|s| s.chars().count()), Ok(4000));
        assert_eq!(OutgoingText::validate(&"가".repeat(4001), false), Err(OutgoingProblem::TooLong));
        assert_eq!(SendRefusal::TooManyAttachments.korean_message(), "한 번에 붙일 수 있는 파일은 4개까지예요.");
    }
}

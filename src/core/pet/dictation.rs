//! Pure state for 받아쓰기 (the OS speech recognizer, ko-KR). The shell feeds recognizer callbacks in; the transcript
//! goes into the input field, the user presses send (dictation never sends by itself). Port of `PetDictation.swift`
//! (on macOS the built-in speech recognition; on Windows the shell's recognizer — the state is the same).

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum DictationPhase {
    #[default]
    Idle,
    RequestingPermission,
    Listening,
    Denied(DictationDenial),
    /// Korean reason, e.g. no recognizer for ko-KR.
    Unavailable(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DictationState {
    pub phase: DictationPhase,
    /// Text already in the field when dictation started (kept as prefix).
    base: String,
    partial: String,
}

impl DictationState {
    pub fn new() -> Self { Self::default() }

    pub fn base(&self) -> &str { &self.base }
    pub fn partial(&self) -> &str { &self.partial }
    pub fn is_listening(&self) -> bool { self.phase == DictationPhase::Listening }
    pub fn text(&self) -> String { Self::join(&self.base, &self.partial) }

    pub fn start(&mut self, existing_text: &str) {
        self.base = existing_text.to_string();
        self.partial.clear();
        self.phase = DictationPhase::RequestingPermission;
    }

    pub fn permission_granted(&mut self) {
        if self.phase == DictationPhase::RequestingPermission { self.phase = DictationPhase::Listening; }
    }

    pub fn permission_denied(&mut self, d: DictationDenial) {
        self.phase = DictationPhase::Denied(d);
        self.partial.clear();
    }

    pub fn unavailable(&mut self, reason: &str) {
        self.phase = DictationPhase::Unavailable(reason.to_string());
        self.partial.clear();
    }

    /// Recognizer hypotheses replace the previous partial; a final result also ends the session.
    pub fn hypothesis(&mut self, t: &str, is_final: bool) {
        if self.phase != DictationPhase::Listening { return; }
        self.partial = t.to_string();
        if is_final { self.stop(); }
    }

    pub fn stop(&mut self) {
        if matches!(self.phase, DictationPhase::Listening | DictationPhase::RequestingPermission) {
            self.base = self.text();
            self.partial.clear();
            self.phase = DictationPhase::Idle;
        }
    }

    fn join(a: &str, b: &str) -> String {
        if a.is_empty() { return b.to_string(); }
        if b.is_empty() { return a.to_string(); }
        let sep = if a.ends_with(' ') || a.ends_with('\n') { "" } else { " " };
        format!("{a}{sep}{b}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DictationDenial { Microphone, Speech }

impl DictationDenial {
    /// Windows wording: the Swift points at macOS 시스템 설정 ▸ 개인정보 보호; here the Windows Settings page.
    pub fn korean_message(self) -> &'static str {
        match self {
            Self::Microphone => "마이크 권한이 꺼져 있어요. Windows 설정 ▸ 개인 정보 및 보안 ▸ 마이크에서 허용해 주세요.",
            Self::Speech => "음성 인식 권한이 꺼져 있어요. Windows 설정 ▸ 개인 정보 및 보안 ▸ 음성에서 허용해 주세요.",
        }
    }
}

#[cfg(test)]
mod tests {
    // PetSettingsAndConnectorsTests.swift / PetDictationTests
    use super::*;

    #[test]
    fn flow_keeps_prefix_and_replaces_partials() {
        let mut d = DictationState::new();
        d.start("오늘");
        assert_eq!(d.phase, DictationPhase::RequestingPermission);
        d.hypothesis("무시", false); // not listening yet
        assert_eq!(d.text(), "오늘");
        d.permission_granted();
        assert!(d.is_listening());
        d.hypothesis("날씨", false);
        d.hypothesis("날씨 어때", false);
        assert_eq!(d.text(), "오늘 날씨 어때");
        d.hypothesis("날씨 어때?", true);
        assert_eq!(d.phase, DictationPhase::Idle);
        assert_eq!(d.text(), "오늘 날씨 어때?");
    }

    #[test]
    fn denial_and_unavailable() {
        let mut d = DictationState::new();
        d.start("");
        d.permission_denied(DictationDenial::Microphone);
        assert_eq!(d.phase, DictationPhase::Denied(DictationDenial::Microphone));
        assert!(!DictationDenial::Speech.korean_message().is_empty());
        d.start("x");
        d.unavailable("한국어 음성 인식을 쓸 수 없어요");
        assert_eq!(d.text(), "x");
    }

    #[test]
    fn stop_midway_keeps_heard_text() {
        let mut d = DictationState::new();
        d.start("");
        d.permission_granted();
        d.hypothesis("안녕", false);
        d.stop();
        assert_eq!(d.text(), "안녕");
    }
}

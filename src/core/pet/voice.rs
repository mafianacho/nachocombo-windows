//! 길게 눌러 말하기 (decided by the owners): press the pet and keep still → a ring fills over about 2 seconds →
//! recording (listening face, sound level) → let go → server dictation (`POST /api/v1/dictation`) → sent as a message
//! by itself, shown as the user's bubble. Let go before the ring is full = the old click (open/close the chat); move
//! more than 3 px = the old drag. Esc or dragging out of the pet while recording = cancel. Then Nacho's answer to a
//! spoken turn: its first sentence is read with the OS's built-in voice (on Windows the shell's speech synthesizer,
//! Korean), never a paid TTS. Port of `PetVoice.swift`.
use super::chat_state::{ChatTurn, ErrorText, TurnStatus};
use std::collections::HashSet;

pub struct PetVoice;

impl PetVoice {
    /// How long the pet is held before recording starts (the one setting for it).
    pub const HOLD_SECONDS: f64 = 2.0;
    /// Movement that turns a press into the old drag.
    pub const DRAG_SLOP: f64 = 3.0;
    /// Shorter recordings are not sent (a tap that slipped past the ring).
    pub const MINIMUM_SECONDS: f64 = 0.5;
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum HoldPhase {
    #[default]
    Idle,
    /// Pressed and still: the ring fills while `talk` is allowed.
    Pressing { start: f64, x: f64, y: f64, talk: bool },
    Dragging,
    Recording { since: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HoldEvent {
    None,
    /// The ring is full: start recording.
    StartRecording,
    /// Let go before the ring was full: the old click.
    Click,
    /// Moved more than 3 px: the old drag (the window follows from now on).
    DragStarted,
    DragEnded,
    /// Let go while recording: hand the audio to the server.
    FinishRecording { seconds: f64 },
    /// Esc, or dragged out of the pet while recording: throw the audio away.
    CancelRecording,
}

/// The pet's press: click, drag, or hold-to-talk. Times in seconds, positions in screen points (either y direction:
/// only distances are used).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PetHoldGesture {
    phase: HoldPhase,
    pub hold_seconds: f64,
}

impl Default for PetHoldGesture {
    fn default() -> Self { Self::new(PetVoice::HOLD_SECONDS) }
}

impl PetHoldGesture {
    pub fn new(hold_seconds: f64) -> Self { Self { phase: HoldPhase::Idle, hold_seconds } }

    pub fn phase(&self) -> HoldPhase { self.phase }

    /// `talk`: hold-to-talk is possible now (signed in). Without it a long press is still a click.
    pub fn down(&mut self, t: f64, x: f64, y: f64, talk: bool) { self.phase = HoldPhase::Pressing { start: t, x, y, talk }; }

    /// 0…1 while the ring fills (0 when it does not).
    pub fn progress(&self, t: f64) -> f64 {
        match self.phase {
            HoldPhase::Pressing { start, talk: true, .. } if self.hold_seconds > 0.0 => ((t - start) / self.hold_seconds).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }

    pub fn is_recording(&self) -> bool { matches!(self.phase, HoldPhase::Recording { .. }) }
    pub fn is_filling(&self) -> bool { matches!(self.phase, HoldPhase::Pressing { talk: true, .. }) }

    /// Called on a timer while pressing: the ring is full → recording.
    pub fn tick(&mut self, t: f64) -> HoldEvent {
        match self.phase {
            HoldPhase::Pressing { start, talk: true, .. } if t - start >= self.hold_seconds => {
                self.phase = HoldPhase::Recording { since: t };
                HoldEvent::StartRecording
            }
            _ => HoldEvent::None,
        }
    }

    /// The mouse moved (screen points). `inside_pet`: still over the pet.
    pub fn move_to(&mut self, t: f64, x: f64, y: f64, inside_pet: bool) -> HoldEvent {
        match self.phase {
            HoldPhase::Pressing { x: x0, y: y0, .. } => {
                // A press that already reached the ring's end is recording; the timer may lag a little behind the clock.
                if self.tick(t) == HoldEvent::StartRecording { return if inside_pet { HoldEvent::StartRecording } else { self.cancel_now() }; }
                if (x - x0).abs() < PetVoice::DRAG_SLOP && (y - y0).abs() < PetVoice::DRAG_SLOP { return HoldEvent::None; }
                self.phase = HoldPhase::Dragging;
                HoldEvent::DragStarted
            }
            HoldPhase::Recording { .. } => if inside_pet { HoldEvent::None } else { self.cancel_now() },
            HoldPhase::Idle | HoldPhase::Dragging => HoldEvent::None,
        }
    }

    pub fn up(&mut self, t: f64) -> HoldEvent {
        let was = std::mem::take(&mut self.phase);
        match was {
            // Let go just as the ring filled (its timer had not fired yet): nothing was recorded and it was no click.
            HoldPhase::Pressing { start, talk, .. } => if talk && t - start >= self.hold_seconds { HoldEvent::None } else { HoldEvent::Click },
            HoldPhase::Dragging => HoldEvent::DragEnded,
            HoldPhase::Recording { since } => HoldEvent::FinishRecording { seconds: (t - since).max(0.0) },
            HoldPhase::Idle => HoldEvent::None,
        }
    }

    /// Esc while recording (or while the ring fills).
    pub fn escape(&mut self) -> HoldEvent {
        match self.phase {
            HoldPhase::Recording { .. } => self.cancel_now(),
            HoldPhase::Pressing { .. } => {
                self.phase = HoldPhase::Idle;
                HoldEvent::None
            }
            _ => HoldEvent::None,
        }
    }

    fn cancel_now(&mut self) -> HoldEvent {
        self.phase = HoldPhase::Idle;
        HoldEvent::CancelRecording
    }
}

/// Korean messages for the voice path (server error codes from nacho-server docs/error-codes.md, spec/dictation.tsp).
pub struct PetVoiceText;

impl PetVoiceText {
    pub const TOO_SHORT: &str = "너무 짧았어요. 나초를 누른 채로 말하고, 다 말하면 손을 떼 주세요.";
    pub const SILENCE: &str = "말소리가 들리지 않았어요. 다시 길게 눌러 말해 주세요.";
    pub const CANCELED: &str = "녹음을 취소했어요.";
    /// Windows wording: the Swift points at macOS 시스템 설정 ▸ 개인정보 보호 및 보안 ▸ 마이크; Windows gates desktop
    /// apps with one switch on the microphone page.
    pub const MIC_DENIED: &str =
        "마이크 권한이 꺼져 있어 들을 수 없어요. Windows 설정 ▸ 개인 정보 및 보안 ▸ 마이크에서 '데스크톱 앱이 마이크에 액세스하도록 허용'을 켜 주세요.";
    pub const OPEN_SETTINGS: &str = "설정 열기";
    pub const UNAVAILABLE: &str = "이 서버는 받아쓰기를 지원하지 않아요.";

    /// A dictation failure (code from the server's error body) in Korean. `too_many_requests` with an RFC 3339
    /// `reason` reads the clock (via [`ErrorText::retry_at`]) to say when it works again.
    pub fn dictation_failure(code: Option<&str>, reason: Option<&str>, max_sec: i64) -> String {
        match code {
            Some("payload_too_large") => format!("말이 너무 길어요. {max_sec}초 안으로 말해 주세요."),
            Some("invalid_input") => format!("녹음을 받지 못했어요. {max_sec}초 안으로 다시 말해 주세요."),
            Some("too_many_requests") => {
                let at = reason.and_then(ErrorText::retry_at).map(|t| format!(" ({t}부터 돼요)")).unwrap_or_default();
                format!("잠시 뒤에 다시 말해 주세요.{at}")
            }
            Some("not_implemented" | "refused_disabled") => Self::UNAVAILABLE.into(),
            Some("unsupported_media_type") => ErrorText::user_message("unsupported_media_type", None, ""),
            Some("upstream_error") => "받아쓰기 서버가 답하지 않았어요. 잠시 뒤 다시 말해 주세요.".into(),
            Some("unauthorized") => "로그인이 필요해요.".into(),
            _ => "받아 적지 못했어요. 다시 말해 주세요.".into(),
        }
    }
}

/// Which turns were started by voice and which of their answers were read aloud.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PetVoiceReplies {
    voice_turns: HashSet<String>,
    spoken: HashSet<String>,
}

impl PetVoiceReplies {
    pub fn new() -> Self { Self::default() }
    pub fn voice_turns(&self) -> &HashSet<String> { &self.voice_turns }
    pub fn spoken(&self) -> &HashSet<String> { &self.spoken }

    /// The user spoke this turn (its client message id).
    pub fn began(&mut self, turn_id: &str) { self.voice_turns.insert(turn_id.to_string()); }

    /// New input, stop, or the setting turned off: nothing pending is read any more.
    pub fn silence_all(&mut self) { self.spoken.extend(self.voice_turns.iter().cloned()); }

    /// The next sentence to read as `(turn_id, text)`: the first sentence of a spoken turn's answer, once it is
    /// complete (or the answer finished), once per turn. Typed turns are never read.
    pub fn next(&mut self, turns: &[ChatTurn], enabled: bool) -> Option<(String, String)> {
        if !enabled {
            self.silence_all();
            return None;
        }
        for t in turns {
            let id = &t.client_msg_id;
            if !self.voice_turns.contains(id) || self.spoken.contains(id) { continue; }
            match t.status {
                TurnStatus::Failed(_) | TurnStatus::Stopped | TurnStatus::Interrupted => {
                    self.spoken.insert(id.clone());
                    continue;
                }
                TurnStatus::Sending | TurnStatus::Streaming | TurnStatus::Done => {}
            }
            let done = t.status == TurnStatus::Done;
            let Some(s) = PetSpeech::first_sentence(&t.assistant_text, done) else {
                if done { self.spoken.insert(id.clone()); } // nothing to read (a card, a task)
                continue;
            };
            self.spoken.insert(id.clone());
            return Some((id.clone(), s));
        }
        None
    }
}

pub struct PetSpeech;

impl PetSpeech {
    pub const MAX_CHARS: usize = 140;

    /// [`PetSpeech::first_sentence_max`] with the default limit of 140 characters.
    pub fn first_sentence(text: &str, is_final: bool) -> Option<String> { Self::first_sentence_max(text, is_final, Self::MAX_CHARS) }

    /// The first sentence of `text` for reading aloud, or `None` while it is not complete yet (`is_final` = the answer
    /// ended). Markdown marks, links and emoji are left out; a very long sentence is cut at a pause.
    pub fn first_sentence_max(text: &str, is_final: bool, max_chars: usize) -> Option<String> {
        let clean = Self::speakable(text);
        if clean.is_empty() { return None; }
        let enders = ['.', '!', '?', '。', '！', '？', '\n'];
        let mut out = String::new();
        let mut ended = false;
        let it: Vec<char> = clean.chars().collect();
        let mut i = 0;
        while i < it.len() {
            let ch = it[i];
            if ch == '\n' {
                ended = !out.trim_matches(is_space).is_empty();
                if ended { break; }
                i += 1;
                continue;
            }
            out.push(ch);
            if enders.contains(&ch) {
                // "1.5" or "3.14" is not an end; an end is followed by a space, a line break or the end of the text.
                let next_is_space = if i + 1 >= it.len() { is_final } else { matches!(it[i + 1], ' ' | '\n' | '"' | '”') };
                if next_is_space {
                    // Keep closing marks: "요." "요!" "?" followed by a quote.
                    while i + 1 < it.len() && enders.contains(&it[i + 1]) && it[i + 1] != '\n' {
                        i += 1;
                        out.push(it[i]);
                    }
                    ended = true;
                    break;
                }
            }
            i += 1;
        }
        if !ended && !is_final { return None; }
        let mut s = out.trim().to_string();
        if s.chars().count() > max_chars {
            let cut: Vec<char> = s.chars().take(max_chars).collect();
            s = match cut.iter().rposition(|c| *c == ',' || *c == ' ') {
                Some(pause) if pause > max_chars / 2 => cut[..pause].iter().collect(),
                _ => cut.iter().collect(),
            };
        }
        if s.is_empty() { None } else { Some(s) }
    }

    /// Text without markdown marks, links and symbols that a voice would read out literally.
    pub(crate) fn speakable(text: &str) -> String {
        // [label](url) → label; bare URLs out.
        let s = Self::strip_bare_urls(&Self::unlink(text));
        let s = s.replace("**", "").replace("__", "").replace('`', "");
        let lines: Vec<String> = s
            .split('\n')
            .map(|line| {
                let mut l = line.trim_matches(is_space);
                while let Some(rest) = l.strip_prefix(['#', '>', '-', '*']) { l = rest.trim_matches(is_space); }
                l.to_string()
            })
            .collect();
        let s: String = lines.join("\n").chars().filter(|c| !is_emoji(*c)).collect();
        s.trim().to_string()
    }

    /// `[label](url)` → `label` (the regex `\[([^\]]*)\]\([^)]*\)`).
    fn unlink(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while let Some(open) = rest.find('[') {
            out.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            let linked = after.find(']').and_then(|close| {
                let url = after[close + 1..].strip_prefix('(')?;
                let end = url.find(')')?;
                Some((&after[..close], &url[end + 1..]))
            });
            match linked {
                Some((label, tail)) => {
                    out.push_str(label);
                    rest = tail;
                }
                None => {
                    out.push('[');
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// Removes `https?://` followed by at least one non-space character, up to the next space.
    fn strip_bare_urls(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        loop {
            let found = ["http://", "https://"].iter().filter_map(|p| rest.find(p).map(|i| (i, p.len()))).min();
            let Some((i, n)) = found else { break };
            let tail = &rest[i + n..];
            let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
            if end == 0 {
                // "http://" with nothing after it is no URL; keep it and look further on.
                out.push_str(&rest[..i + n]);
            } else {
                out.push_str(&rest[..i]);
            }
            rest = &tail[end..];
        }
        out.push_str(rest);
        out
    }
}

/// Swift `CharacterSet.whitespaces`: spaces and tabs, not line breaks.
fn is_space(c: char) -> bool { c == '\t' || (c.is_whitespace() && !matches!(c, '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}')) }

/// Swift's `isEmojiPresentation || (isEmoji && value > 0x2000)`: the Unicode `Emoji` property above U+2000 (std has
/// no Unicode property tables, so the ranges are listed; the supplementary planes are taken as whole blocks — a few
/// unassigned or text symbols there are dropped too, harmless for speech). Keeps ©, ®, digits, # and * (below
/// U+2000) and plain text.
fn is_emoji(c: char) -> bool {
    const RANGES: &[(u32, u32)] = &[
        (0x203C, 0x203C), (0x2049, 0x2049), (0x2122, 0x2122), (0x2139, 0x2139), (0x2194, 0x2199), (0x21A9, 0x21AA),
        (0x231A, 0x231B), (0x2328, 0x2328), (0x23CF, 0x23CF), (0x23E9, 0x23F3), (0x23F8, 0x23FA), (0x24C2, 0x24C2),
        (0x25AA, 0x25AB), (0x25B6, 0x25B6), (0x25C0, 0x25C0), (0x25FB, 0x25FE), (0x2600, 0x2604), (0x260E, 0x260E),
        (0x2611, 0x2611), (0x2614, 0x2615), (0x2618, 0x2618), (0x261D, 0x261D), (0x2620, 0x2620), (0x2622, 0x2623),
        (0x2626, 0x2626), (0x262A, 0x262A), (0x262E, 0x262F), (0x2638, 0x263A), (0x2640, 0x2640), (0x2642, 0x2642),
        (0x2648, 0x2653), (0x265F, 0x2660), (0x2663, 0x2663), (0x2665, 0x2666), (0x2668, 0x2668), (0x267B, 0x267B),
        (0x267E, 0x267F), (0x2692, 0x2697), (0x2699, 0x2699), (0x269B, 0x269C), (0x26A0, 0x26A1), (0x26A7, 0x26A7),
        (0x26AA, 0x26AB), (0x26B0, 0x26B1), (0x26BD, 0x26BE), (0x26C4, 0x26C5), (0x26C8, 0x26C8), (0x26CE, 0x26CF),
        (0x26D1, 0x26D1), (0x26D3, 0x26D4), (0x26E9, 0x26EA), (0x26F0, 0x26F5), (0x26F7, 0x26FA), (0x26FD, 0x26FD),
        (0x2702, 0x2702), (0x2705, 0x2705), (0x2708, 0x270D), (0x270F, 0x270F), (0x2712, 0x2712), (0x2714, 0x2714),
        (0x2716, 0x2716), (0x271D, 0x271D), (0x2721, 0x2721), (0x2728, 0x2728), (0x2733, 0x2734), (0x2744, 0x2744),
        (0x2747, 0x2747), (0x274C, 0x274C), (0x274E, 0x274E), (0x2753, 0x2755), (0x2757, 0x2757), (0x2763, 0x2764),
        (0x2795, 0x2797), (0x27A1, 0x27A1), (0x27B0, 0x27B0), (0x27BF, 0x27BF), (0x2934, 0x2935), (0x2B05, 0x2B07),
        (0x2B1B, 0x2B1C), (0x2B50, 0x2B50), (0x2B55, 0x2B55), (0x3030, 0x3030), (0x303D, 0x303D), (0x3297, 0x3297),
        (0x3299, 0x3299), (0x1F004, 0x1F004), (0x1F0CF, 0x1F0CF), (0x1F170, 0x1F171), (0x1F17E, 0x1F17F),
        (0x1F18E, 0x1F18E), (0x1F191, 0x1F19A), (0x1F1E6, 0x1F1FF), (0x1F201, 0x1F202), (0x1F21A, 0x1F21A),
        (0x1F22F, 0x1F22F), (0x1F232, 0x1F23A), (0x1F250, 0x1F251), (0x1F300, 0x1F64F), (0x1F680, 0x1F6FF),
        (0x1F7E0, 0x1F7F0), (0x1F900, 0x1F9FF), (0x1FA70, 0x1FAFF),
    ];
    let v = c as u32;
    v > 0x2000 && RANGES.iter().any(|&(lo, hi)| (lo..=hi).contains(&v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use HoldEvent as E;

    // PetVoiceChoiceTests.swift / PetHoldGestureTests

    #[test]
    fn let_go_before_the_ring_is_full_is_the_old_click() {
        let mut g = PetHoldGesture::default();
        g.down(10.0, 100.0, 100.0, true);
        assert_eq!(g.tick(10.5), E::None);
        assert!((g.progress(11.0) - 0.5).abs() < 0.0001, "half way at 1 s");
        assert_eq!(g.tick(11.9), E::None);
        assert_eq!(g.up(11.95), E::Click);
        assert_eq!(g.phase(), HoldPhase::Idle);
    }

    #[test]
    fn hold_two_seconds_records_until_let_go() {
        assert_eq!(PetVoice::HOLD_SECONDS, 2.0);
        let mut g = PetHoldGesture::default();
        g.down(0.0, 100.0, 100.0, true);
        assert_eq!(g.progress(2.5), 1.0);
        assert_eq!(g.tick(2.0), E::StartRecording);
        assert!(g.is_recording());
        assert_eq!(g.tick(3.0), E::None, "starts once");
        assert_eq!(g.move_to(3.0, 140.0, 100.0, true), E::None, "moving inside the pet while talking is no drag");
        assert_eq!(g.up(5.25), E::FinishRecording { seconds: 3.25 });
        assert_eq!(g.phase(), HoldPhase::Idle);
    }

    #[test]
    fn move_three_points_is_the_old_drag() {
        let mut g = PetHoldGesture::default();
        g.down(0.0, 100.0, 100.0, true);
        assert_eq!(g.move_to(0.1, 102.0, 101.0, true), E::None, "under 3 px");
        assert_eq!(g.move_to(0.2, 103.0, 100.0, true), E::DragStarted);
        assert_eq!(g.progress(1.5), 0.0, "no ring while dragging");
        assert_eq!(g.tick(3.0), E::None, "a long drag never records");
        assert_eq!(g.up(3.0), E::DragEnded);
    }

    #[test]
    fn drag_out_or_esc_cancels_the_recording() {
        let mut g = PetHoldGesture::default();
        g.down(0.0, 100.0, 100.0, true);
        let _ = g.tick(2.0);
        assert_eq!(g.move_to(3.0, 400.0, 100.0, false), E::CancelRecording);
        assert_eq!(g.up(4.0), E::None, "letting go after the cancel sends nothing");
        g.down(10.0, 100.0, 100.0, true);
        let _ = g.tick(12.0);
        assert_eq!(g.escape(), E::CancelRecording);
        assert_eq!(g.up(13.0), E::None);
    }

    #[test]
    fn moved_past_the_end_of_the_ring_before_the_timer_fired() {
        let mut g = PetHoldGesture::default();
        g.down(0.0, 100.0, 100.0, true);
        assert_eq!(g.move_to(2.1, 101.0, 100.0, true), E::StartRecording);
        assert!(g.is_recording());
        let mut h = PetHoldGesture::default();
        h.down(0.0, 100.0, 100.0, true);
        assert_eq!(h.up(2.05), E::None, "let go just as it filled: neither a click nor a recording");
    }

    #[test]
    fn signed_out_a_long_press_is_still_a_click() {
        let mut g = PetHoldGesture::default();
        g.down(0.0, 0.0, 0.0, false);
        assert_eq!(g.progress(1.0), 0.0);
        assert!(!g.is_filling());
        assert_eq!(g.tick(5.0), E::None);
        assert_eq!(g.up(5.0), E::Click);
    }

    // PetVoiceChoiceTests.swift / PetSpokenReplyTests

    fn turn(id: &str, text: &str, status: TurnStatus) -> ChatTurn {
        let mut t = ChatTurn::new(id, "질문");
        t.assistant_text = text.into();
        t.status = status;
        t
    }

    fn text(r: Option<(String, String)>) -> Option<String> { r.map(|(_, s)| s) }

    #[test]
    fn only_spoken_turns_are_read_once_first_sentence_when_complete() {
        let mut r = PetVoiceReplies::new();
        r.began("v1");
        assert_eq!(r.next(&[turn("v1", "오늘은 맑", TurnStatus::Streaming)], true), None, "not a whole sentence yet");
        let streaming = turn("v1", "오늘은 맑고 따뜻해요. 오후에는 바람이", TurnStatus::Streaming);
        assert_eq!(text(r.next(&[streaming], true)).as_deref(), Some("오늘은 맑고 따뜻해요."));
        assert_eq!(r.next(&[turn("v1", "오늘은 맑고 따뜻해요. 오후에는 바람이 불어요.", TurnStatus::Done)], true), None, "once per turn");
        assert_eq!(r.next(&[turn("t1", "글로 물은 답이에요.", TurnStatus::Done)], true), None, "typed turns are never read");
        r.began("v2");
        assert_eq!(r.next(&[turn("v2", "네", TurnStatus::Done)], true), Some(("v2".into(), "네".into())), "a short answer without a full stop, once done");
    }

    #[test]
    fn new_input_stop_or_setting_off_silences() {
        let mut r = PetVoiceReplies::new();
        r.began("v1");
        r.silence_all();
        assert_eq!(r.next(&[turn("v1", "답이에요.", TurnStatus::Done)], true), None);
        r.began("v2");
        assert_eq!(r.next(&[turn("v2", "답이에요.", TurnStatus::Done)], false), None);
        assert_eq!(r.next(&[turn("v2", "답이에요.", TurnStatus::Done)], true), None, "turned off meanwhile: that answer stays unread");
        r.began("v3");
        assert_eq!(r.next(&[turn("v3", "", TurnStatus::Failed("x".into()))], true), None);
        assert_eq!(r.next(&[turn("v3", "늦은 글.", TurnStatus::Done)], true), None, "a failed turn is dropped");
    }

    #[test]
    fn first_sentence_cleans_markdown_links_and_numbers() {
        let first = |s: &str, f: bool| PetSpeech::first_sentence(s, f);
        assert_eq!(first("**결과**: 1.5배 빨라졌어요! 자세한 건 아래를 보세요.", false).as_deref(), Some("결과: 1.5배 빨라졌어요!"));
        assert_eq!(first("## 요약\n- 3곳을 찾았어요", true).as_deref(), Some("요약"));
        assert_eq!(first("[숯불집](https://example.com)이 좋아요. 다음은", false).as_deref(), Some("숯불집이 좋아요."));
        assert_eq!(first("https://example.com 여기 있어요?! 네", false).as_deref(), Some("여기 있어요?!"));
        assert_eq!(first("   ", true), None);
        let long = "가나다라마바사, ".repeat(30) + "끝.";
        assert!(first(&long, true).unwrap().chars().count() <= 140);
        // Not in Swift: emoji and an unclosed bracket.
        assert_eq!(first("좋아요 😀👍 정말요. 다음", false).as_deref(), Some("좋아요  정말요."));
        assert_eq!(first("[메모 남겼어요.", true).as_deref(), Some("[메모 남겼어요."));
    }

    #[test]
    fn dictation_errors_in_korean() {
        let f = PetVoiceText::dictation_failure;
        assert_eq!(f(Some("payload_too_large"), None, 60), "말이 너무 길어요. 60초 안으로 말해 주세요.");
        assert_eq!(f(Some("not_implemented"), None, 60), PetVoiceText::UNAVAILABLE);
        assert_eq!(f(Some("too_many_requests"), None, 60), "잠시 뒤에 다시 말해 주세요.");
        assert_eq!(f(Some("upstream_error"), None, 60), "받아쓰기 서버가 답하지 않았어요. 잠시 뒤 다시 말해 주세요.");
        assert_eq!(f(None, None, 60), "받아 적지 못했어요. 다시 말해 주세요.");
    }
}

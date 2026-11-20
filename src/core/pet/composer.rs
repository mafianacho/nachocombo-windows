//! The chat's input row: what the send button does, and the number keys that answer a card while the field is empty.
//! Port of `sendClicked` / `cardDigit` of `PetChatWindow.swift`.
use super::card_flow::{CardStep, PetCardFlow};
use super::card_rules::PetCardKey;
use super::server_models::PetCard;

/// What a press of the send button or Enter does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendPlan {
    /// Empty field, nothing attached: nothing.
    Nothing,
    /// Nacho is answering and the field is empty: the button is "stop".
    Stop,
    /// Not answering: send now.
    Send,
    /// Nacho is answering and there is text: it waits and goes out when the answer ends.
    Queue,
}

/// Enter with text while Nacho answers never stops the answer; stopping is the stop button (empty field) and Esc.
pub fn plan_send(busy: bool, text: &str, has_pending_files: bool, from_return: bool) -> SendPlan {
    let has = !text.trim().is_empty() || has_pending_files;
    match (busy, has) {
        (true, false) => if from_return { SendPlan::Nothing } else { SendPlan::Stop },
        (true, true) => SendPlan::Queue,
        (false, true) => SendPlan::Send,
        (false, false) => SendPlan::Nothing,
    }
}

/// What the round button shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendButton { Stop, Send { armed: bool, queues: bool } }

pub fn send_button(busy: bool, has_text: bool) -> SendButton {
    if busy && !has_text { SendButton::Stop } else { SendButton::Send { armed: has_text, queues: busy } }
}

/// What the field should do after [`DigitHold::changed`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DigitEdit {
    /// Leave the field as typed.
    Keep,
    /// The digit marked a card row: the field is emptied.
    Clear,
    /// The digit was the start of a message: the field gets this text.
    Replace(String),
}

/// A single number typed into the empty field marks (or ticks) that row of the newest waiting card instead of starting a
/// message. When more text follows, the number was the start of a message: it goes back in front and the mark goes away.
#[derive(Default)]
pub struct DigitHold {
    held: Option<(usize, String)>,
}

impl DigitHold {
    pub fn new() -> Self { Self::default() }

    pub fn reset(&mut self) { self.held = None; }

    /// Call on every change of the field's text. `card` is the newest card waiting for an answer here.
    pub fn changed(&mut self, text: &str, card: Option<&PetCard>, flow: &mut PetCardFlow) -> DigitEdit {
        if let Some((n, card_id)) = self.held.take() {
            if text.is_empty() { return DigitEdit::Keep; }
            let single = single_digit(text);
            if let (Some(m), Some(c)) = (single, card) {
                if c.id == card_id && flow.key(PetCardKey::Digit(m as i64), c) != CardStep::Pass {
                    // Another number: another row (one choice) or one more tick (several).
                    self.held = Some((m, c.id.clone()));
                    return DigitEdit::Clear;
                }
            }
            if let Some(c) = card.filter(|c| c.id == card_id) { flow.key_undo(c, n); }
            return DigitEdit::Replace(format!("{n}{text}"));
        }
        let (Some(n), Some(c)) = (single_digit(text), card) else { return DigitEdit::Keep };
        if flow.key(PetCardKey::Digit(n as i64), c) == CardStep::Pass { return DigitEdit::Keep; }
        self.held = Some((n, c.id.clone()));
        DigitEdit::Clear
    }
}

fn single_digit(text: &str) -> Option<usize> {
    let mut it = text.chars();
    let c = it.next()?;
    if it.next().is_some() { return None; }
    c.to_digit(10).filter(|d| *d >= 1).map(|d| d as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::pet::card_rules::PetCardLocal;
    use crate::core::pet::server_models::PetCardOption;

    fn card() -> PetCard { PetCard::new("c1", "choice", "?", vec![PetCardOption::new("a", "가"), PetCardOption::new("b", "나")]) }

    #[test]
    fn enter_with_text_while_busy_queues_and_never_stops() {
        assert_eq!(plan_send(true, "다음 질문", false, true), SendPlan::Queue);
        assert_eq!(plan_send(true, "  ", false, true), SendPlan::Nothing, "Enter on an empty field does not stop the answer");
        assert_eq!(plan_send(true, "", false, false), SendPlan::Stop, "the stop button does");
        assert_eq!(plan_send(true, "", true, true), SendPlan::Queue, "a waiting file counts as something to send");
        assert_eq!(plan_send(false, "안녕", false, true), SendPlan::Send);
        assert_eq!(plan_send(false, "", false, false), SendPlan::Nothing);
    }

    #[test]
    fn the_round_button_follows_busy_and_text() {
        assert_eq!(send_button(true, false), SendButton::Stop);
        assert_eq!(send_button(true, true), SendButton::Send { armed: true, queues: true });
        assert_eq!(send_button(false, false), SendButton::Send { armed: false, queues: false });
    }

    #[test]
    fn a_digit_marks_the_row_and_a_following_character_makes_it_a_message_again() {
        let (mut f, mut h, c) = (PetCardFlow::new(), DigitHold::new(), card());
        assert_eq!(h.changed("2", Some(&c), &mut f), DigitEdit::Clear);
        assert_eq!(f.local("c1").highlighted, Some(2));
        // The user keeps typing "시에 만나": the field shows "시" → the mark goes and "2" is put back in front.
        assert_eq!(h.changed("시", Some(&c), &mut f), DigitEdit::Replace("2시".into()));
        assert_eq!(f.local("c1").highlighted, None);
        assert_eq!(h.changed("2시", Some(&c), &mut f), DigitEdit::Keep, "an ordinary message goes on typing");
    }

    #[test]
    fn another_digit_moves_the_mark_and_no_card_leaves_digits_alone() {
        let (mut f, mut h, c) = (PetCardFlow::new(), DigitHold::new(), card());
        assert_eq!(h.changed("1", Some(&c), &mut f), DigitEdit::Clear);
        assert_eq!(h.changed("2", Some(&c), &mut f), DigitEdit::Clear);
        assert_eq!(f.local("c1").highlighted, Some(2));
        let mut none_f = PetCardFlow::new();
        assert_eq!(DigitHold::new().changed("1", None, &mut none_f), DigitEdit::Keep);
        assert_eq!(DigitHold::new().changed("9", Some(&c), &mut f), DigitEdit::Keep, "no row 9");
        assert_eq!(f.local("c1").highlighted, Some(2));
        let _ = PetCardLocal::default();
    }
}

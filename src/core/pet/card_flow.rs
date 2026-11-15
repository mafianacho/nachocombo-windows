//! Answering cards in the chat: what the app keeps per card while the user works with it (ticks, a row marked with a
//! number key, the answer on its way, a failure for 다시 시도) and the steps between a click or key and the answer that
//! goes to the server. The shell only draws the cards and sends what [`PetCardFlow`] hands back. Port of the card part
//! of `NachoPetModule+Voice.swift`. The approval card's drawing is shared with the desktop-actions work (W3): both use
//! `crate::pet_cards::draw_card` and this flow.
use super::card_rules::{PetCardAnswer, PetCardKey, PetCardKeyResult, PetCardKeys, PetCardLocal, PetCardRules, PetCardStyle};
use super::server_models::PetCard;
use std::collections::BTreeMap;

/// What the shell does after a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardStep {
    /// Nothing to send (a tick or a mark changed: draw again).
    Redraw,
    /// Send this answer for the card (`PetSession::answer_card`), then call [`PetCardFlow::finished`].
    Send { card_id: String, answer: PetCardAnswer },
    /// Not for a card.
    Pass,
}

#[derive(Default)]
pub struct PetCardFlow {
    local: BTreeMap<String, PetCardLocal>,
}

impl PetCardFlow {
    pub fn new() -> Self { Self::default() }

    /// The card's local state for [`super::transcript::PetTranscript::items`].
    pub fn locals(&self) -> &BTreeMap<String, PetCardLocal> { &self.local }

    pub fn local(&self, id: &str) -> PetCardLocal { self.local.get(id).cloned().unwrap_or_default() }

    /// A row was clicked (or its number + Enter): an answer, or a tick on a multiple choice.
    pub fn row(&mut self, card: &PetCard, number: usize) -> CardStep {
        if !card.is_pending() { return CardStep::Pass; }
        let rows = PetCardRules::rows(card);
        if number < 1 || number > rows.len() { return CardStep::Pass; }
        let row = &rows[number - 1];
        if PetCardStyle::of(card) == PetCardStyle::Multiple {
            if let Some(o) = &row.option_id {
                self.toggle_tick(&card.id, o);
                return CardStep::Redraw;
            }
        }
        match &row.answer {
            Some(a) => self.begin(card, a.clone()),
            None => CardStep::Pass,
        }
    }

    /// 확정 of a multiple choice.
    pub fn confirm(&mut self, card: &PetCard) -> CardStep {
        match PetCardRules::confirm(card, &self.local(&card.id).ticked) {
            Some(a) => self.begin(card, a),
            None => CardStep::Pass,
        }
    }

    /// 다시 시도 after a failure: the same answer again.
    pub fn retry(&mut self, card: &PetCard) -> CardStep {
        match self.local(&card.id).last_answer {
            Some(a) => self.begin(card, a),
            None => CardStep::Pass,
        }
    }

    /// An answer chosen by the shell itself (the approval card's 실행 with a rule, a button of a plan card).
    pub fn answer(&mut self, card: &PetCard, a: PetCardAnswer) -> CardStep { self.begin(card, a) }

    fn toggle_tick(&mut self, id: &str, option: &str) {
        let l = self.local.entry(id.to_string()).or_default();
        if let Some(i) = l.ticked.iter().position(|o| o == option) { l.ticked.remove(i); } else { l.ticked.push(option.to_string()); }
        l.failure = None;
    }

    /// One answer at a time per card: a second press while it is on its way sends nothing.
    fn begin(&mut self, card: &PetCard, a: PetCardAnswer) -> CardStep {
        let l = self.local.entry(card.id.clone()).or_default();
        if l.sending.is_some() { return CardStep::Pass; }
        l.sending = Some(a.clone());
        l.last_answer = Some(a.clone());
        l.failure = None;
        l.highlighted = None;
        CardStep::Send { card_id: card.id.clone(), answer: a }
    }

    /// The server's reply to a sent answer. A refusal is kept for 다시 시도; success forgets the card's local state and
    /// returns the text to send as the user's next message (ask_choice and suggestions).
    pub fn finished(&mut self, card_id: &str, answer: &PetCardAnswer, refusal: Option<String>) -> Option<String> {
        let l = self.local.entry(card_id.to_string()).or_default();
        l.sending = None;
        match refusal {
            Some(r) => { l.failure = Some(r); None }
            None => { self.local.remove(card_id); answer.follow_up_text.clone() }
        }
    }

    /// A number key, Enter or Esc while the input field is empty, for the newest card waiting here.
    pub fn key(&mut self, key: PetCardKey, card: &PetCard) -> CardStep {
        let local = self.local(&card.id);
        match PetCardKeys::handle(key, card, &local) {
            PetCardKeyResult::Pass => CardStep::Pass,
            PetCardKeyResult::Highlight(n) => { self.local.entry(card.id.clone()).or_default().highlighted = Some(n); CardStep::Redraw }
            PetCardKeyResult::Toggle(o) => { self.toggle_tick(&card.id, &o); CardStep::Redraw }
            PetCardKeyResult::Answer(a) => self.begin(card, a),
            PetCardKeyResult::Clear => {
                let l = self.local.entry(card.id.clone()).or_default();
                l.highlighted = None;
                l.ticked.clear();
                CardStep::Redraw
            }
        }
    }

    /// Undoes a number key that turned out to be the start of a message ("2시에…"): the mark or the tick goes away.
    pub fn key_undo(&mut self, card: &PetCard, n: usize) {
        let Some(l) = self.local.get_mut(&card.id) else { return };
        if PetCardStyle::of(card) == PetCardStyle::Multiple {
            let rows = PetCardRules::rows(card);
            if n >= 1 && n <= rows.len() {
                if let Some(o) = &rows[n - 1].option_id { if let Some(i) = l.ticked.iter().position(|t| t == o) { l.ticked.remove(i); } }
            }
        } else if l.highlighted == Some(n) {
            l.highlighted = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::pet::server_models::PetCardOption;

    fn choice(id: &str, multiple: bool) -> PetCard {
        let mut c = PetCard::new(id, "choice", "어느 걸로 할까요?", vec![PetCardOption::new("a", "가"), PetCardOption::new("b", "나")]);
        c.multiple = multiple;
        c
    }

    #[test]
    fn a_click_sends_once_and_a_second_press_while_sending_sends_nothing() {
        let (mut f, c) = (PetCardFlow::new(), choice("c1", false));
        let first = f.row(&c, 1);
        assert!(matches!(first, CardStep::Send { .. }));
        assert_eq!(f.row(&c, 2), CardStep::Pass, "no double click while it is on its way");
        assert!(f.local("c1").sending.is_some());
    }

    #[test]
    fn a_refusal_is_kept_for_retry_and_success_forgets_the_card_and_returns_the_follow_up() {
        let (mut f, c) = (PetCardFlow::new(), choice("c1", false));
        let CardStep::Send { answer, .. } = f.row(&c, 1) else { panic!("sent") };
        assert_eq!(f.finished("c1", &answer, Some("서버에 연결되어 있지 않아요.".into())), None);
        assert_eq!(f.local("c1").failure.as_deref(), Some("서버에 연결되어 있지 않아요."));
        assert!(matches!(f.retry(&c), CardStep::Send { .. }), "다시 시도 sends the same answer");
        let follow = f.finished("c1", &answer, None);
        assert_eq!(follow, answer.follow_up_text, "a chosen text goes on as the next message");
        assert_eq!(f.local("c1"), PetCardLocal::default(), "answered: nothing is kept");
    }

    #[test]
    fn a_multiple_choice_ticks_then_confirms() {
        let (mut f, c) = (PetCardFlow::new(), choice("m1", true));
        assert_eq!(f.row(&c, 1), CardStep::Redraw);
        assert_eq!(f.row(&c, 2), CardStep::Redraw);
        assert_eq!(f.row(&c, 1), CardStep::Redraw, "a second click unticks");
        assert_eq!(f.local("m1").ticked, vec!["b".to_string()]);
        let CardStep::Send { answer, .. } = f.confirm(&c) else { panic!("confirm sends") };
        assert_eq!(answer.option_ids, vec!["b".to_string()]);
    }

    #[test]
    fn number_key_marks_enter_answers_escape_clears_and_undo_takes_the_mark_back() {
        let (mut f, c) = (PetCardFlow::new(), choice("c1", false));
        assert_eq!(f.key(PetCardKey::Digit(2), &c), CardStep::Redraw);
        assert_eq!(f.local("c1").highlighted, Some(2));
        assert_eq!(f.key(PetCardKey::Escape, &c), CardStep::Redraw);
        assert_eq!(f.local("c1").highlighted, None);
        f.key(PetCardKey::Digit(2), &c);
        f.key_undo(&c, 2);
        assert_eq!(f.local("c1").highlighted, None, "\"2시에…\" was a message, not a mark");
        f.key(PetCardKey::Digit(1), &c);
        assert!(matches!(f.key(PetCardKey::Enter, &c), CardStep::Send { .. }));
        assert_eq!(f.key(PetCardKey::Digit(3), &c), CardStep::Pass, "no row 3: the key is for the text field");
    }

    #[test]
    fn an_answered_card_is_not_touched() {
        let (mut f, mut c) = (PetCardFlow::new(), choice("c1", false));
        c.status = "answered".into();
        assert_eq!(f.row(&c, 1), CardStep::Pass);
        assert_eq!(f.key(PetCardKey::Digit(1), &c), CardStep::Pass);
    }
}

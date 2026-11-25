//! Poll votes (docs/PROTOCOL_NOTES.md, Polls): counts follow MESSAGE_POLL_VOTE_ADD / _REMOVE and
//! my own clicks, which are counted at once and restored when the request fails.

use super::AppState;
use crate::model::{Id, Poll, PollAnswerCount};

/// The answers I voted for.
pub fn my_votes(p: &Poll) -> Vec<u32> {
    p.results.iter().flat_map(|r| &r.answer_counts).filter(|c| c.me_voted).map(|c| c.id).collect()
}

/// One vote added or removed. A poll without results gets them: the first event is all we know.
pub(super) fn apply_vote(p: &mut Poll, answer_id: u32, add: bool, me: bool) {
    let results = p.results.get_or_insert_with(Default::default);
    if results.is_finalized {
        return;
    }
    let i = match results.answer_counts.iter().position(|c| c.id == answer_id) {
        Some(i) => i,
        None => {
            results.answer_counts.push(PollAnswerCount { id: answer_id, ..Default::default() });
            results.answer_counts.len() - 1
        }
    };
    let c = &mut results.answer_counts[i];
    if me {
        // Mine count once: the click already counted it, so its echo changes nothing.
        if c.me_voted == add {
            return;
        }
        c.me_voted = add;
    }
    c.count = if add { c.count + 1 } else { c.count.saturating_sub(1) };
}

impl AppState {
    /// My votes on a poll become exactly `answers` (counts follow). Returns the votes I had, to
    /// restore when the request fails; None when the poll is not loaded.
    pub fn set_my_poll_votes(&mut self, ch: Id, mid: Id, answers: &[u32]) -> Option<Vec<u32>> {
        let p = self.message_mut(ch, mid)?.poll.as_deref_mut()?;
        let before = my_votes(p);
        for id in before.iter().filter(|id| !answers.contains(id)) {
            apply_vote(p, *id, false, true);
        }
        for id in answers {
            apply_vote(p, *id, true, true);
        }
        Some(before)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{GatewayEvent, Message, MessageUpdate, PollAnswer, PollResults, User};

    const ME: Id = Id(1);
    const CH: Id = Id(10);
    const MID: Id = Id(20);

    fn state(results: Option<PollResults>) -> AppState {
        let mut s = AppState { me: Some(User { id: ME, ..Default::default() }), ..Default::default() };
        let answers = (1..=3).map(|answer_id| PollAnswer { answer_id, ..Default::default() }).collect();
        let poll = Poll { answers, results, ..Default::default() };
        s.messages.entry(CH).or_default().msgs.push(Message { id: MID, channel_id: CH, poll: Some(Box::new(poll)), ..Default::default() });
        s
    }

    fn counts(s: &AppState) -> Vec<(u32, u32, bool)> {
        let p = s.messages[&CH].msgs[0].poll.as_deref().unwrap();
        let mut v: Vec<_> = p.results.iter().flat_map(|r| &r.answer_counts).map(|c| (c.id, c.count, c.me_voted)).collect();
        v.sort();
        v
    }

    fn vote(s: &mut AppState, user: Id, answer_id: u32, add: bool) {
        s.apply_gateway(GatewayEvent::PollVote { channel_id: CH, message_id: MID, user_id: user, answer_id, add }, None);
    }

    #[test]
    fn other_users_votes_count_and_missing_results_are_created() {
        let mut s = state(None);
        vote(&mut s, Id(2), 2, true);
        vote(&mut s, Id(3), 2, true);
        vote(&mut s, Id(3), 1, true);
        vote(&mut s, Id(2), 2, false);
        assert_eq!(counts(&s), [(1, 1, false), (2, 1, false)]);
        // Removing more than there is never underflows.
        vote(&mut s, Id(3), 1, false);
        vote(&mut s, Id(3), 1, false);
        assert_eq!(counts(&s)[0], (1, 0, false));
    }

    #[test]
    fn my_vote_counts_once_with_its_echo() {
        let mut s = state(None);
        assert_eq!(s.set_my_poll_votes(CH, MID, &[2]), Some(vec![]));
        assert_eq!(counts(&s), [(2, 1, true)]);
        vote(&mut s, ME, 2, true);
        assert_eq!(counts(&s), [(2, 1, true)], "the echo changes nothing");
        // Removing: the click decrements and clears, the echo changes nothing.
        assert_eq!(s.set_my_poll_votes(CH, MID, &[]), Some(vec![2]));
        vote(&mut s, ME, 2, false);
        assert_eq!(counts(&s), [(2, 0, false)]);
        // A vote of mine from another session still counts.
        vote(&mut s, ME, 3, true);
        assert_eq!(counts(&s), [(2, 0, false), (3, 1, true)]);
    }

    #[test]
    fn restoring_the_previous_votes_gives_the_counts_back() {
        let results = PollResults {
            is_finalized: false,
            answer_counts: vec![PollAnswerCount { id: 1, count: 4, me_voted: true }, PollAnswerCount { id: 2, count: 2, me_voted: false }],
        };
        let mut s = state(Some(results));
        let before = counts(&s);
        let prev = s.set_my_poll_votes(CH, MID, &[2, 3]).unwrap();
        assert_eq!(prev, [1]);
        assert_eq!(counts(&s), [(1, 3, false), (2, 3, true), (3, 1, true)]);
        s.set_my_poll_votes(CH, MID, &prev);
        let after: Vec<_> = counts(&s).into_iter().filter(|c| c.0 != 3).collect();
        assert_eq!(after, before);
        assert_eq!(counts(&s)[2], (3, 0, false), "an answer that was never counted stays at 0");
        assert_eq!(s.set_my_poll_votes(CH, Id(99), &[1]), None);
    }

    #[test]
    fn a_failed_vote_restores_the_counts_and_says_so() {
        let mut s = state(None);
        let restore = s.set_my_poll_votes(CH, MID, &[1]).unwrap();
        let toast = s.apply_chat(crate::backend::ChatEvent::PollVoteFailed { channel_id: CH, message_id: MID, restore, error: "Forbidden".into() });
        assert_eq!(toast.as_deref(), Some("Could not vote: Forbidden"));
        assert_eq!(counts(&s), [(1, 0, false)]);
    }

    #[test]
    fn a_finalized_poll_ignores_votes() {
        let mut s = state(Some(PollResults { is_finalized: true, answer_counts: vec![PollAnswerCount { id: 1, count: 9, me_voted: false }] }));
        vote(&mut s, Id(2), 1, true);
        vote(&mut s, Id(2), 2, true);
        assert_eq!(counts(&s), [(1, 9, false)]);
    }

    #[test]
    fn an_update_without_results_keeps_the_known_ones() {
        let mut s = state(Some(PollResults { is_finalized: false, answer_counts: vec![PollAnswerCount { id: 1, count: 3, me_voted: true }] }));
        let poll = Poll { results: None, answers: vec![PollAnswer { answer_id: 1, ..Default::default() }], ..Default::default() };
        s.apply_gateway(GatewayEvent::MessageUpdate(Box::new(MessageUpdate { id: MID, channel_id: CH, poll: Some(Box::new(poll)), ..Default::default() })), None);
        assert_eq!(counts(&s), [(1, 3, true)]);
        assert_eq!(s.messages[&CH].msgs[0].poll.as_deref().unwrap().answers.len(), 1, "the rest of the poll is replaced");
        // New results replace the old.
        let fresh = Poll { results: Some(PollResults { is_finalized: true, answer_counts: vec![PollAnswerCount { id: 1, count: 5, me_voted: true }] }), ..Default::default() };
        s.apply_gateway(GatewayEvent::MessageUpdate(Box::new(MessageUpdate { id: MID, channel_id: CH, poll: Some(Box::new(fresh)), ..Default::default() })), None);
        assert_eq!(counts(&s), [(1, 5, true)]);
    }
}

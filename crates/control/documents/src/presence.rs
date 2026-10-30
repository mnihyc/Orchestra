//! Who is in a document, and who is writing in which block.
//!
//! Who has a document open is the presence of its feed, which the socket
//! keeps for every feed alike. What a document adds is who is writing, and
//! in which block: while a person who may edit writes, their client says so
//! every few seconds with the block their caret is in, and the others show
//! them beside that block for a few seconds after the last time, which is
//! what keeps two people out of the same paragraph. It is never stored and
//! never told again to somebody who comes later, and never shown to the
//! person who wrote it.

use std::collections::HashMap;

use orch_realtime_contract::typing::{TYPING_SEND_MIN_MILLIS, TYPING_SHOWN_MILLIS};
use orch_types::{Id, Time};

fn millis_between(earlier: Time, later: Time) -> i64 {
    later.unix_millis().saturating_sub(earlier.unix_millis())
}

/// Who is writing, in the documents of one process.
#[derive(Debug, Clone, Default)]
pub struct Writers {
    /// The last notice that was passed on, by document and person: when, and where.
    last: HashMap<(Id, Id), (Time, Option<Id>)>,
}

impl Writers {
    /// Nobody is writing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A notice that `user` is writing in `block` of `document` arrived at
    /// `now`. Whether it is to be passed on to the others: not when the last
    /// one of that person in that document was passed on less than the
    /// shortest time apart that a client sends them, which only a client
    /// that does not keep to it is told.
    pub fn note(&mut self, document: Id, user: Id, block: Option<Id>, now: Time) -> bool {
        let key = (document, user);
        if let Some((passed, _)) = self.last.get(&key)
            && millis_between(*passed, now) < TYPING_SEND_MIN_MILLIS
        {
            return false;
        }
        self.last.insert(key, (now, block));
        true
    }

    /// Who is shown as writing in `document` at `now`, and in which block:
    /// those whose last notice is less than the time one is shown for.
    #[must_use]
    pub fn shown(&self, document: Id, now: Time) -> Vec<(Id, Option<Id>)> {
        let mut shown: Vec<(Id, Option<Id>)> = self
            .last
            .iter()
            .filter(|((of, _), (when, _))| {
                *of == document && millis_between(*when, now) < TYPING_SHOWN_MILLIS
            })
            .map(|((_, user), (_, block))| (*user, *block))
            .collect();
        shown.sort_unstable();
        shown
    }

    /// Forgets the notices that are too old to be shown any more.
    pub fn forget_old(&mut self, now: Time) {
        self.last
            .retain(|_, (when, _)| millis_between(*when, now) < TYPING_SHOWN_MILLIS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    fn at(millis: i64) -> Time {
        let base: Time = "2026-10-08T17:00:00.000Z".parse().unwrap();
        Time::new(base.get() + chrono::Duration::milliseconds(millis))
    }

    #[test]
    fn a_notice_that_comes_sooner_than_three_seconds_after_the_last_is_dropped() {
        let (doc, ada, block) = (new_id(), new_id(), new_id());
        let mut writers = Writers::new();
        assert!(writers.note(doc, ada, Some(block), at(0)));
        assert!(!writers.note(doc, ada, Some(block), at(2_999)));
        assert!(writers.note(doc, ada, Some(block), at(3_000)));
        let other_doc = new_id();
        assert!(
            writers.note(other_doc, ada, None, at(3_001)),
            "another document is another count"
        );
    }

    #[test]
    fn a_person_is_shown_beside_their_block_for_six_seconds_after_the_last_notice() {
        let (doc, ada, bo, one, two) = (new_id(), new_id(), new_id(), new_id(), new_id());
        let mut writers = Writers::new();
        writers.note(doc, ada, Some(one), at(0));
        writers.note(doc, bo, Some(two), at(4_000));
        let mut both = vec![(ada, Some(one)), (bo, Some(two))];
        both.sort_unstable();
        assert_eq!(writers.shown(doc, at(5_999)), both);
        assert_eq!(writers.shown(doc, at(6_000)), [(bo, Some(two))]);
        assert_eq!(writers.shown(doc, at(10_000)), []);
        assert_eq!(writers.shown(new_id(), at(1)), []);
        writers.forget_old(at(10_000));
        assert!(writers.last.is_empty());
    }
}

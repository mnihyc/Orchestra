//! Typing notices in chats, and who is writing in which block of a document.
//!
//! While a person types in a chat's composer, or writes in a document,
//! their client sends `typing` for the feed, at most once every three
//! seconds ([`TYPING_SEND_MIN_MILLIS`]). The server passes it to the
//! feed's other subscribers as an `event` frame whose data is a
//! [`TypingNotice`], and a client shows the person as typing for six
//! seconds after the last notice ([`TYPING_SHOWN_MILLIS`]). Typing is
//! never stored and never replayed, is never shown to the person who
//! typed, and is not a thing of a thread's feed, where what is typed is
//! addressed to an agent.
//!
//! In a document the notice says in which block the caret is, which is
//! what keeps two people out of the same paragraph.

use crate::pacing::{RateLimit, millis_between};
use orch_api_contract::users::UserRef;
use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The shortest time, in milliseconds, between two `typing` frames that a
/// client sends for one feed, and that a server lets through; a frame that
/// comes sooner is dropped. Three seconds is slow enough to cost nothing on
/// a slow link, and with [`TYPING_SHOWN_MILLIS`] twice it, one notice that
/// is lost does not make the indicator flicker.
pub const TYPING_SEND_MIN_MILLIS: i64 = 3_000;
/// How long, in milliseconds, a client shows a person as typing after the
/// last notice of them.
pub const TYPING_SHOWN_MILLIS: i64 = 6_000;

/// What the other subscribers of a chat or a document are told, as the
/// data of an `event` frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "what", rename_all = "snake_case")]
pub enum TypingNotice {
    /// A person is writing.
    Typing {
        /// Who.
        user: UserRef,
        /// In a document, the block their caret is in; left out in a chat,
        /// and when the sender named none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<Id>,
    },
}

/// Lets a server drop the `typing` frames of one connection on one feed
/// that come too soon after the last it let through.
#[must_use]
pub const fn send_limit() -> RateLimit {
    RateLimit::new(TYPING_SEND_MIN_MILLIS)
}

/// Whether the notice of `user` is to be shown to `subscriber`: to
/// everyone but the person who typed.
#[must_use]
pub fn shown_to(user: Id, subscriber: Id) -> bool {
    user != subscriber
}

/// Who a client shows as typing, in one chat or one document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Typists {
    last: BTreeMap<Id, (Time, Option<Id>)>,
}

impl Typists {
    /// Nobody is typing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A notice arrived at `now`: `user` is typing, in the block `at` of
    /// a document.
    pub fn note(&mut self, user: Id, at: Option<Id>, now: Time) {
        self.last.insert(user, (now, at));
    }

    /// Those who are shown as typing at `now`, and in which block: the
    /// people whose last notice is less than [`TYPING_SHOWN_MILLIS`] old.
    #[must_use]
    pub fn shown(&self, now: Time) -> Vec<(Id, Option<Id>)> {
        self.last
            .iter()
            .filter(|(_, (when, _))| millis_between(*when, now) < TYPING_SHOWN_MILLIS)
            .map(|(user, (_, at))| (*user, *at))
            .collect()
    }

    /// Forgets those whose notice is old enough not to be shown.
    pub fn forget_old(&mut self, now: Time) {
        self.last
            .retain(|_, (when, _)| millis_between(*when, now) < TYPING_SHOWN_MILLIS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pacing::moment;
    use serde_json::json;

    fn id(last: u8) -> Id {
        format!("0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a{last:02x}")
            .parse()
            .unwrap()
    }

    fn person() -> serde_json::Value {
        json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01", "kind": "human", "username": "bo",
            "display_name": "Bo", "avatar": { "text": "B", "color": 2 }
        })
    }

    #[test]
    fn who_is_typing_in_a_chat_is_told_as_the_person_alone() {
        let written = json!({ "what": "typing", "user": person() });
        let notice: TypingNotice = serde_json::from_value(written.clone()).unwrap();
        let TypingNotice::Typing { user, at } = &notice;
        assert_eq!(user.username, "bo");
        assert_eq!(*at, None);
        assert_eq!(serde_json::to_value(&notice).unwrap(), written);
    }

    #[test]
    fn who_is_writing_in_a_document_is_told_with_the_block_their_caret_is_in() {
        let written = json!({ "what": "typing", "user": person(), "at": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a07" });
        let notice: TypingNotice = serde_json::from_value(written.clone()).unwrap();
        let TypingNotice::Typing { at, .. } = &notice;
        assert_eq!(*at, Some(id(7)));
        assert_eq!(serde_json::to_value(&notice).unwrap(), written);
    }

    #[test]
    fn a_notice_of_another_sort_is_not_typing() {
        assert!(
            serde_json::from_value::<TypingNotice>(json!({ "what": "weather", "user": person() }))
                .is_err()
        );
    }

    #[test]
    fn a_person_is_shown_as_typing_for_six_seconds_after_the_last_notice() {
        let mut typists = Typists::new();
        typists.note(id(1), None, moment(0));
        assert_eq!(typists.shown(moment(5_999)), [(id(1), None)]);
        assert!(typists.shown(moment(6_000)).is_empty());
        // Another notice makes it six seconds from then.
        typists.note(id(1), None, moment(3_000));
        assert_eq!(typists.shown(moment(8_999)), [(id(1), None)]);
        assert!(typists.shown(moment(9_000)).is_empty());
    }

    #[test]
    fn a_document_shows_in_which_block_each_person_is_writing_and_follows_the_caret() {
        let mut typists = Typists::new();
        typists.note(id(1), Some(id(10)), moment(0));
        typists.note(id(2), Some(id(11)), moment(1_000));
        typists.note(id(1), Some(id(12)), moment(2_000));
        assert_eq!(
            typists.shown(moment(3_000)),
            [(id(1), Some(id(12))), (id(2), Some(id(11)))]
        );
    }

    #[test]
    fn those_whose_notice_is_old_are_forgotten() {
        let mut typists = Typists::new();
        typists.note(id(1), None, moment(0));
        typists.note(id(2), None, moment(5_000));
        typists.forget_old(moment(7_000));
        assert_eq!(typists.shown(moment(7_000)), [(id(2), None)]);
    }

    #[test]
    fn typing_is_never_shown_to_the_person_who_typed() {
        assert!(!shown_to(id(1), id(1)));
        assert!(shown_to(id(1), id(2)));
    }

    #[test]
    fn more_than_one_frame_in_three_seconds_is_dropped() {
        let mut limit = send_limit();
        assert!(limit.admit(moment(0)));
        assert!(!limit.admit(moment(1_500)));
        assert!(!limit.admit(moment(2_999)));
        assert!(limit.admit(moment(3_000)));
    }
}

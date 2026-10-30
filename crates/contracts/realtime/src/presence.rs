//! Visible readers.
//!
//! On the feed of a thread, of a chat and of a document, the server says
//! who is looking. A person is looking when at least one of their
//! connections has the thing visible: the client sends `visible` when the
//! view becomes visible or hidden, by the page's visibility and by the
//! route. The server sends `presence`, the whole list of those who are
//! looking, right after `subscribed` and then whenever the list changes,
//! at most once a second ([`PRESENCE_INTERVAL_MILLIS`], kept with a
//! [`Throttle`](crate::pacing::Throttle)).
//!
//! A thread's presence says nothing about typing: what is typed there is
//! addressed to an agent. A connection that only inspects a chat (it holds
//! `directs.view` and is not in the chat) is left out of the chat's
//! presence, and the server does not tell this tracker about it.

use crate::pacing::millis_between;
use orch_api_contract::users::UserRef;
use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The shortest time, in milliseconds, between two `presence` frames to
/// the subscribers of one feed. A tab that is switched back and forth many
/// times a second is reported once, with the list as it then is.
pub const PRESENCE_INTERVAL_MILLIS: i64 = 1_000;

/// One person who is looking, as a `presence` frame lists them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PresenceUser {
    /// The person.
    pub user: UserRef,
    /// Since when they have been looking: the moment the first of their
    /// connections had the thing visible, kept while any does.
    pub since: Time,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Reader {
    since: Time,
    connections: BTreeSet<String>,
}

/// Who is looking at one feed, for the connections of one server.
///
/// A person is in it from the moment one of their connections says the
/// feed is visible until the last of them says it is hidden or closes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VisibleSet {
    readers: BTreeMap<Id, Reader>,
}

impl VisibleSet {
    /// A set in which nobody is looking.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The connection `connection` of `user` says that the feed is, or is
    /// no longer, visible there. `true` when the people who are looking
    /// changed by it.
    pub fn set_visible(&mut self, user: Id, connection: &str, visible: bool, now: Time) -> bool {
        if visible {
            match self.readers.get_mut(&user) {
                Some(reader) => {
                    reader.connections.insert(connection.to_owned());
                    false
                }
                None => {
                    let connections = BTreeSet::from([connection.to_owned()]);
                    self.readers.insert(
                        user,
                        Reader {
                            since: now,
                            connections,
                        },
                    );
                    true
                }
            }
        } else {
            let Some(reader) = self.readers.get_mut(&user) else {
                return false;
            };
            reader.connections.remove(connection);
            if reader.connections.is_empty() {
                self.readers.remove(&user);
                true
            } else {
                false
            }
        }
    }

    /// The connection `connection` closed, or stopped following the feed.
    /// `true` when the people who are looking changed by it.
    pub fn connection_gone(&mut self, connection: &str) -> bool {
        let before = self.readers.len();
        self.readers.retain(|_, reader| {
            reader.connections.remove(connection);
            !reader.connections.is_empty()
        });
        self.readers.len() != before
    }

    /// Those who are looking, with since when, the longest first.
    #[must_use]
    pub fn readers(&self) -> Vec<(Id, Time)> {
        let mut list: Vec<(Id, Time)> = self
            .readers
            .iter()
            .map(|(user, reader)| (*user, reader.since))
            .collect();
        list.sort_by_key(|(user, since)| (*since, *user));
        list
    }

    /// Whether nobody is looking.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.readers.is_empty()
    }

    /// For how long, in milliseconds as of `now`, `user` has been looking;
    /// `None` when they are not.
    #[must_use]
    pub fn looking_for_millis(&self, user: Id, now: Time) -> Option<i64> {
        self.readers
            .get(&user)
            .map(|reader| millis_between(reader.since, now))
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

    #[test]
    fn a_person_is_looking_from_the_first_connection_that_shows_the_feed() {
        let mut set = VisibleSet::new();
        assert!(set.set_visible(id(1), "c1", true, moment(1_000)));
        assert!(!set.set_visible(id(1), "c2", true, moment(5_000)));
        // Since the moment the first connection showed it, not the second.
        assert_eq!(set.readers(), [(id(1), moment(1_000))]);
        assert_eq!(set.looking_for_millis(id(1), moment(4_000)), Some(3_000));
    }

    #[test]
    fn a_person_with_two_tabs_is_looking_until_the_last_of_them_hides_it() {
        let mut set = VisibleSet::new();
        set.set_visible(id(1), "c1", true, moment(0));
        set.set_visible(id(1), "c2", true, moment(1_000));
        assert!(!set.set_visible(id(1), "c1", false, moment(2_000)));
        assert_eq!(set.readers().len(), 1);
        assert!(set.set_visible(id(1), "c2", false, moment(3_000)));
        assert!(set.is_empty());
    }

    #[test]
    fn a_person_who_looks_again_has_a_new_since() {
        let mut set = VisibleSet::new();
        set.set_visible(id(1), "c1", true, moment(0));
        set.set_visible(id(1), "c1", false, moment(1_000));
        set.set_visible(id(1), "c1", true, moment(9_000));
        assert_eq!(set.readers(), [(id(1), moment(9_000))]);
    }

    #[test]
    fn a_connection_that_closes_takes_its_person_away_when_it_was_their_last() {
        let mut set = VisibleSet::new();
        set.set_visible(id(1), "c1", true, moment(0));
        set.set_visible(id(2), "c2", true, moment(500));
        set.set_visible(id(2), "c3", true, moment(600));
        assert!(set.connection_gone("c1"));
        assert!(!set.connection_gone("c2"));
        assert_eq!(set.readers(), [(id(2), moment(500))]);
        assert!(!set.connection_gone("c9"));
        assert!(set.connection_gone("c3"));
        assert!(set.is_empty());
    }

    #[test]
    fn hiding_what_was_never_shown_changes_nothing() {
        let mut set = VisibleSet::new();
        assert!(!set.set_visible(id(1), "c1", false, moment(0)));
        assert!(set.is_empty());
    }

    #[test]
    fn those_who_look_are_listed_the_longest_first() {
        let mut set = VisibleSet::new();
        set.set_visible(id(3), "c3", true, moment(300));
        set.set_visible(id(1), "c1", true, moment(100));
        set.set_visible(id(2), "c2", true, moment(100));
        let users: Vec<Id> = set.readers().into_iter().map(|(user, _)| user).collect();
        assert_eq!(users, [id(1), id(2), id(3)]);
    }

    #[test]
    fn a_reader_is_written_with_the_person_and_since_when() {
        let written = json!({
            "user": {
                "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01", "kind": "human", "username": "bo",
                "display_name": "Bo", "avatar": { "text": "B", "color": 2 }
            },
            "since": "2026-10-08T17:04:05.123Z"
        });
        let reader: PresenceUser = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(reader.user.username, "bo");
        assert_eq!(serde_json::to_value(&reader).unwrap(), written);
    }
}

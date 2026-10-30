//! Mention resolution for people who can read a chat.
//!
//! `@username` in a text names a person, and `@all` names everyone who follows
//! the chat. A name is a mention only when it is a person who can read the
//! chat; any other `@` word stays what it was, text. Mentions are resolved
//! when a message is written and again when it is edited, and nobody is named
//! by their own message.

use crate::chats::Loaded;
use crate::{Acting, Error};
use orch_api_contract::chats::ChatPart;
use orch_db::Conn;
use orch_db::queries::chats as rows;
use orch_types::{Id, Time};

/// The most different people one message names. More are left as text: a
/// message that names hundreds is a way to be loud, and each name costs a
/// question to the access engine.
pub const MENTIONS_MAX: usize = 20;

/// The shortest and the longest username there is.
const NAME_LENGTHS: std::ops::RangeInclusive<usize> = 3..=32;

/// The word that names everyone who follows a chat.
const EVERYONE: &str = "all";

/// Who a message names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Named {
    /// The people it names, in the order it names them, who can read the
    /// chat; not its author.
    pub users: Vec<Id>,
    /// Whether it says `@all`.
    pub everyone: bool,
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// What stands before a mention: nothing, or something that is not part of a
/// word, so that `bo@example.org` names nobody.
fn may_precede(c: char) -> bool {
    !(c.is_alphanumeric() || matches!(c, '_' | '@' | '.' | '-'))
}

/// The usernames a text names with `@`, lowercase and each once in the order
/// of their first mention, and whether it says `@all`.
#[must_use]
pub fn scan(text: &str) -> (Vec<String>, bool) {
    let chars: Vec<char> = text.chars().collect();
    let mut names: Vec<String> = Vec::new();
    let mut everyone = false;
    let mut at = 0;
    while at < chars.len() {
        if chars[at] != '@' || (at > 0 && !may_precede(chars[at - 1])) {
            at += 1;
            continue;
        }
        let start = at + 1;
        let mut end = start;
        while end < chars.len() && is_name_char(chars[end]) {
            end += 1;
        }
        let name: String = chars[start..end].iter().collect::<String>().to_lowercase();
        if name == EVERYONE {
            everyone = true;
        } else if NAME_LENGTHS.contains(&name.len())
            && name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric())
            && !names.contains(&name)
        {
            names.push(name);
        }
        at = end.max(at + 1);
    }
    (names, everyone)
}

/// The text of the parts that people write.
fn texts(parts: &[ChatPart]) -> impl Iterator<Item = &str> {
    parts.iter().filter_map(|part| match part {
        ChatPart::Text { text } => Some(text.as_str()),
        _ => None,
    })
}

/// Who the parts of a message name: the people whose username they give who
/// can read the chat, and whether they say `@all`. `author` is never named.
pub async fn resolve(
    conn: &mut Conn,
    act: &Acting<'_>,
    chat: &Loaded,
    parts: &[ChatPart],
    author: Option<Id>,
) -> Result<Named, Error> {
    let mut names: Vec<String> = Vec::new();
    let mut everyone = false;
    for text in texts(parts) {
        let (found, all) = scan(text);
        everyone |= all;
        for name in found {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names.truncate(MENTIONS_MAX);
    // An agent's broad words remain literal. A human audience is frozen with this write.
    everyone &= author.is_some();
    let found = rows::standing_by_username(conn, chat.row.org_id, &names).await?;
    let candidates: Vec<Id> = names
        .iter()
        .filter_map(|name| {
            found
                .iter()
                .find(|person| person.username.to_lowercase() == *name)
        })
        .map(|person| person.id)
        .filter(|id| Some(*id) != author)
        .collect();
    let readers = orch_authz::chats::readers_among(
        conn,
        &chat.row,
        &chat.members,
        &candidates,
        Time::new(act.now),
    )
    .await?;
    let mut users: Vec<Id> = candidates
        .into_iter()
        .filter(|id| readers.contains(id))
        .collect();
    if author.is_some() {
        users.extend(crate::audiences::resolve(conn, act, chat, parts).await?);
    }
    if everyone {
        users.extend(crate::audiences::followers(conn, act, chat).await?);
    }
    users.sort_unstable();
    users.dedup();
    Ok(Named { users, everyone })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_after_an_at_sign_is_a_mention_once_in_the_order_it_comes() {
        let (names, everyone) = scan("Hi @Ada and @bob-2, @ada again; ask (@cyd).");
        assert_eq!(names, ["ada", "bob-2", "cyd"]);
        assert!(!everyone);
    }

    #[test]
    fn at_all_names_everyone_and_is_no_username() {
        let (names, everyone) = scan("@ALL please look, @all.");
        assert!(everyone && names.is_empty());
    }

    #[test]
    fn an_address_or_a_word_that_is_too_short_or_too_long_names_nobody() {
        let long = format!("@{}", "x".repeat(33));
        for text in [
            "write to bo@example.org",
            "@bo",
            &long,
            "e@ada",
            "mail@@ada",
            "a.@ada",
        ] {
            let (names, everyone) = scan(text);
            assert!(names.is_empty() && !everyone, "{text}");
        }
    }

    #[test]
    fn a_name_inside_a_longer_word_or_after_punctuation_that_belongs_to_a_word_is_not_one() {
        assert!(scan("foo-@ada").0.is_empty());
        assert_eq!(scan("-> @ada").0, ["ada"]);
        assert_eq!(scan("\n@ada").0, ["ada"]);
    }
}

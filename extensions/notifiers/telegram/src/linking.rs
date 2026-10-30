//! A person linked to the bot: a link that carries a code, read back when they press Start; the bot is asked only while a code waits, so nothing of a system has to be reachable from outside.
//!
//! The address a person opens is `https://t.me/<bot_username>?start=<code>`.
//! Pressing Start there sends the bot the message `/start <code>`. Nothing
//! listens for it: while a code waits, the worker reads what the bot was sent
//! ([`poll`]) and takes the messages of that form that were sent in a private
//! chat, whose code is made only of the characters a link may carry. The chat
//! is what the notifier learned, and the name of whoever pressed Start is the
//! label, so that the person sees on their route that it is theirs.
//!
//! The reading continues from where the last ended: the cursor is the number of
//! the first update not read yet, which also tells Telegram that the ones
//! before it are taken.

use crate::bot::{Bot, Update};
use orch_notifier_contract::delivery::{DeliverError, LinkPoll, LinkStarted};
use serde_json::json;
use std::time::Duration;

/// The longest code a link may carry.
const LONGEST_CODE: usize = 64;

/// The address that opens the bot with `code`, for a bot whose name is
/// `bot_username`.
#[must_use]
pub fn link_url(bot_username: &str, code: &str) -> Option<String> {
    let name = bot_username.trim().trim_start_matches('@');
    (!name.is_empty() && is_code(code)).then(|| format!("https://t.me/{name}?start={code}"))
}

/// Whether `text` can be carried by a link: letters, digits, `_` and `-`, at
/// most 64 of them.
fn is_code(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= LONGEST_CODE
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The code in a message that is `/start <code>`, in a private chat's words:
/// `/start` or `/start@name`, then the code.
fn code_of(text: &str) -> Option<&str> {
    let (command, rest) = text.trim().split_once(char::is_whitespace)?;
    let command = command.split('@').next()?;
    let code = rest.trim();
    (command == "/start" && is_code(code)).then_some(code)
}

/// Who followed a link among what the bot was sent.
#[must_use]
pub fn followers(updates: &[Update]) -> Vec<LinkStarted> {
    updates
        .iter()
        .filter_map(|update| {
            let message = update.message.as_ref()?;
            let code = code_of(message.text.as_deref()?)?;
            let person = message.from.as_ref()?;
            (message.chat.kind == "private").then(|| LinkStarted {
                code: code.to_owned(),
                address: json!({ "chat_id": message.chat.id }),
                label: person.label(),
            })
        })
        .collect()
}

/// Reads what the bot was sent since `cursor`, waiting up to `wait` for
/// something to arrive, and says who followed a link and where the next reading
/// continues.
pub async fn poll(
    bot: &Bot,
    cursor: Option<String>,
    wait: Duration,
) -> Result<LinkPoll, DeliverError> {
    let offset = cursor.as_deref().and_then(|text| text.parse::<i64>().ok());
    let updates = bot.get_updates(offset, wait).await?;
    let next = updates
        .iter()
        .map(|update| update.update_id)
        .max()
        .map(|newest| (newest + 1).to_string())
        .or(cursor);
    Ok(LinkPoll {
        started: followers(&updates),
        cursor: next,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::{Chat, Message, User};

    fn update(
        id: i64,
        chat_kind: &str,
        chat: i64,
        text: Option<&str>,
        from: Option<User>,
    ) -> Update {
        Update {
            update_id: id,
            message: Some(Message {
                from,
                chat: Chat {
                    id: chat,
                    kind: chat_kind.to_owned(),
                },
                text: text.map(str::to_owned),
            }),
        }
    }

    fn dana() -> Option<User> {
        Some(User {
            id: 42,
            first_name: "Dana".into(),
            last_name: None,
            username: Some("dana".into()),
        })
    }

    #[test]
    fn the_address_that_opens_the_bot_carries_the_code() {
        assert_eq!(
            link_url("orch_bot", "AbC-123_x").as_deref(),
            Some("https://t.me/orch_bot?start=AbC-123_x")
        );
        assert_eq!(
            link_url("@orch_bot", "c0de").as_deref(),
            Some("https://t.me/orch_bot?start=c0de")
        );
        assert_eq!(
            link_url("", "c0de"),
            None,
            "a bot whose name is not known gives no link"
        );
        assert_eq!(link_url("orch_bot", "has space"), None);
        assert_eq!(
            link_url("orch_bot", &"x".repeat(65)),
            None,
            "a start parameter is up to 64 characters"
        );
        assert!(link_url("orch_bot", &"x".repeat(64)).is_some());
    }

    #[test]
    fn pressing_start_in_a_private_chat_with_a_code_is_following_the_link() {
        let updates = [
            update(10, "private", 42, Some("/start c0de-1"), dana()),
            update(11, "private", 42, Some("/start@orch_bot c0de-2"), dana()),
            update(12, "private", 42, Some("hello"), dana()),
            update(13, "private", 42, Some("/start"), dana()),
            update(14, "group", -100, Some("/start c0de-3"), dana()),
            update(15, "private", 42, Some("/start not a code"), dana()),
            update(16, "private", 42, Some("/startx c0de-4"), dana()),
            update(17, "private", 42, None, dana()),
            update(18, "private", 42, Some("/start c0de-5"), None),
            Update {
                update_id: 19,
                message: None,
            },
        ];
        let followed = followers(&updates);
        let codes: Vec<_> = followed.iter().map(|each| each.code.as_str()).collect();
        assert_eq!(codes, ["c0de-1", "c0de-2"]);
        assert_eq!(followed[0].address, json!({ "chat_id": 42 }));
        assert_eq!(followed[0].label, "Dana");
    }
}

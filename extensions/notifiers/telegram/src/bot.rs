//! The Bot API as this notifier uses it: who the bot is, sending a message, and asking what the bot was sent; its limits, and the wait it is told to keep.
//!
//! Three methods and no others: `getMe`, to know the bot behind a token;
//! `sendMessage`, to write plain text to a chat; and `getUpdates`, to read what
//! the bot was sent, as one waiting request. Every call is a `POST` of JSON to
//! `https://api.telegram.org/bot<token>/<method>`, answered by an envelope that
//! says whether it went well, and, when it did not, why, and how long to wait
//! when the bot is sending too much.
//!
//! The token is in the address, so an error never shows the address: what is
//! reported is what Telegram said, or that it could not be reached.

use orch_notifier_contract::delivery::DeliverError;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;
use url::Url;

/// Where the Bot API is. A system that reaches Telegram reaches this and
/// nothing else of it.
pub const API_BASE: &str = "https://api.telegram.org";

/// The most characters a message takes, which the Bot API counts after it has
/// read the entities of a text; a plain text has none.
pub const MAX_MESSAGE_CHARS: u32 = 4096;

/// How long a call that does not wait on purpose may take.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a connection may take to be made.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How much longer than the wait it asked for a reading of what the bot was
/// sent is given before it is given up on: the time to get an answer back.
pub const READING_GRACE: Duration = Duration::from_secs(10);

/// The longest a reading may be told to wait: the Bot API takes up to a few
/// minutes, and a proxy between the worker and Telegram leaves a quiet
/// connection for less than one.
pub const LONGEST_WAIT_SECS: u64 = 50;

/// The most updates one reading takes.
pub const UPDATES_PER_READING: u32 = 100;

/// The longest a wait may be that the far side asked for, in seconds: a day,
/// so that an answer cannot park a delivery for good.
pub const MAX_RETRY_AFTER_SECS: u32 = 86_400;

/// What the Bot API says of a call.
#[derive(Debug, Deserialize)]
struct Envelope {
    ok: bool,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    error_code: Option<i64>,
    #[serde(default)]
    parameters: Option<ResponseParameters>,
}

/// Why a call failed, beside the words.
#[derive(Debug, Deserialize)]
struct ResponseParameters {
    #[serde(default)]
    retry_after: Option<u32>,
}

/// Who a bot is.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Me {
    /// Its identifier.
    pub id: i64,
    /// Its name in links, without the `@`.
    #[serde(default)]
    pub username: Option<String>,
    /// Its name.
    #[serde(default)]
    pub first_name: String,
}

/// A person, as Telegram names them beside what they sent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct User {
    /// Their identifier.
    pub id: i64,
    /// Their first name.
    #[serde(default)]
    pub first_name: String,
    /// Their last name.
    #[serde(default)]
    pub last_name: Option<String>,
    /// Their name in links.
    #[serde(default)]
    pub username: Option<String>,
}

impl User {
    /// How they are named to somebody who looks at the route: their name, or,
    /// where they gave none, their username.
    #[must_use]
    pub fn label(&self) -> String {
        let name = [Some(self.first_name.as_str()), self.last_name.as_deref()]
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        match (&name, &self.username) {
            (name, _) if !name.is_empty() => name.clone(),
            (_, Some(username)) => format!("@{username}"),
            _ => self.id.to_string(),
        }
    }
}

/// The chat a message was sent in.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Chat {
    /// Its identifier, which is what the bot writes to.
    pub id: i64,
    /// `private`, `group`, `supergroup` or `channel`.
    #[serde(rename = "type")]
    pub kind: String,
}

/// A message the bot was sent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Message {
    /// Who sent it.
    #[serde(default)]
    pub from: Option<User>,
    /// Where.
    pub chat: Chat,
    /// What it says, where it is text.
    #[serde(default)]
    pub text: Option<String>,
}

/// One thing the bot was sent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Update {
    /// Its number, which only grows.
    pub update_id: i64,
    /// The message, where it is one.
    #[serde(default)]
    pub message: Option<Message>,
}

/// A bot, reached with its token.
#[derive(Clone)]
pub struct Bot {
    client: reqwest::Client,
    base: Url,
    token: String,
}

impl std::fmt::Debug for Bot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bot")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

/// What a failure to reach the Bot API means: it may pass.
fn unreachable(error: reqwest::Error) -> DeliverError {
    let error = error.without_url();
    if error.is_timeout() {
        DeliverError::temporary("timeout", "Telegram did not answer in time")
    } else if error.is_connect() {
        DeliverError::temporary(
            "unreachable",
            &format!("Telegram cannot be connected to: {error}"),
        )
    } else {
        DeliverError::temporary("network", &format!("the call to Telegram failed: {error}"))
    }
}

/// What the Bot API refused with, and what that comes to.
///
/// Telegram answers with a code beside the HTTP status, and with words. A
/// bot that was blocked, a user who is deactivated and a chat that is not
/// found are all the end of the route; a bot that is not allowed (a token
/// that is not one) or a text that is wrong fail the delivery; flood control
/// names the time to wait; and a conflict, which is another reading of what the
/// bot was sent still waiting, and a server error, pass.
fn refusal(method: &str, status: u16, envelope: &Envelope) -> DeliverError {
    let words = envelope
        .description
        .clone()
        .unwrap_or_else(|| format!("answered {status}"));
    let words: String = words.split_whitespace().collect::<Vec<_>>().join(" ");
    let code = envelope.error_code.unwrap_or_else(|| i64::from(status));
    if let Some(seconds) = envelope
        .parameters
        .as_ref()
        .and_then(|each| each.retry_after)
    {
        return DeliverError::after("retry_after", &words, seconds.min(MAX_RETRY_AFTER_SECS));
    }
    let lower = words.to_ascii_lowercase();
    match code {
        429 => DeliverError::temporary("too_many_requests", &words),
        401 | 404 => DeliverError::refused(
            "unauthorized",
            &format!("the token is not a bot's: {words}"),
        ),
        403 if method == "sendMessage" => DeliverError::gone("forbidden", &words),
        400 if method == "sendMessage"
            && (lower.contains("chat not found") || lower.contains("user not found")) =>
        {
            DeliverError::gone("chat_not_found", &words)
        }
        409 => DeliverError::temporary("conflict", &words),
        500..=599 => DeliverError::temporary("server_error", &words),
        _ => DeliverError::refused("bad_request", &words),
    }
}

impl Bot {
    /// A bot at `base` (the Bot API, or a stand-in for it) with this token.
    pub fn new(client: reqwest::Client, base: &Url, token: &str) -> Self {
        Self {
            client,
            base: base.clone(),
            token: token.to_owned(),
        }
    }

    /// The client every bot is reached with: no redirects, since the Bot API
    /// does not redirect, and an answer is never followed anywhere.
    pub fn client() -> Result<reqwest::Client, DeliverError> {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent("orch-telegram")
            .build()
            .map_err(|error| DeliverError::refused("client", &error.without_url().to_string()))
    }

    async fn call(
        &self,
        method: &str,
        parameters: Value,
        timeout: Duration,
    ) -> Result<Value, DeliverError> {
        let mut address = self.base.clone();
        address
            .path_segments_mut()
            .map_err(|()| {
                DeliverError::refused(
                    "invalid_config",
                    "the address of the Bot API cannot carry a path",
                )
            })?
            .pop_if_empty()
            .push(&format!("bot{}", self.token))
            .push(method);
        let response = self
            .client
            .post(address)
            .timeout(timeout)
            .json(&parameters)
            .send()
            .await
            .map_err(unreachable)?;
        let status = response.status().as_u16();
        let envelope: Envelope = match response.json().await {
            Ok(envelope) => envelope,
            Err(_) => {
                return Err(if (500..600).contains(&status) {
                    DeliverError::temporary("server_error", &format!("Telegram answered {status}"))
                } else {
                    DeliverError::temporary(
                        "protocol",
                        &format!("Telegram answered {status} without an envelope"),
                    )
                });
            }
        };
        if envelope.ok {
            Ok(envelope.result)
        } else {
            Err(refusal(method, status, &envelope))
        }
    }

    /// Who the bot is.
    pub async fn get_me(&self) -> Result<Me, DeliverError> {
        let result = self.call("getMe", json!({}), CALL_TIMEOUT).await?;
        serde_json::from_value(result).map_err(|error| {
            DeliverError::temporary(
                "protocol",
                &format!("getMe answered something else: {error}"),
            )
        })
    }

    /// Writes `text`, as plain text, to the chat. A text longer than a message
    /// takes is cut here as a last resort; the worker has cut it at a line
    /// already.
    pub async fn send_message(&self, chat_id: i64, text: &str) -> Result<(), DeliverError> {
        let text: String = text.chars().take(MAX_MESSAGE_CHARS as usize).collect();
        self.call(
            "sendMessage",
            json!({ "chat_id": chat_id, "text": text, "link_preview_options": { "is_disabled": true } }),
            CALL_TIMEOUT,
        )
        .await
        .map(|_| ())
    }

    /// What the bot was sent from the update numbered `offset` on, waiting up to
    /// `wait` for something to arrive. Asking from an offset confirms every
    /// update before it.
    pub async fn get_updates(
        &self,
        offset: Option<i64>,
        wait: Duration,
    ) -> Result<Vec<Update>, DeliverError> {
        let wait_secs = wait.as_secs().min(LONGEST_WAIT_SECS);
        let mut parameters = json!({
            "timeout": wait_secs, "limit": UPDATES_PER_READING, "allowed_updates": ["message"]
        });
        if let Some(offset) = offset {
            parameters["offset"] = json!(offset);
        }
        let result = self
            .call(
                "getUpdates",
                parameters,
                Duration::from_secs(wait_secs) + READING_GRACE,
            )
            .await?;
        serde_json::from_value(result).map_err(|error| {
            DeliverError::temporary(
                "protocol",
                &format!("getUpdates answered something else: {error}"),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(code: i64, words: &str, retry_after: Option<u32>) -> Envelope {
        Envelope {
            ok: false,
            result: Value::Null,
            description: Some(words.to_owned()),
            error_code: Some(code),
            parameters: retry_after.map(|retry_after| ResponseParameters {
                retry_after: Some(retry_after),
            }),
        }
    }

    #[test]
    fn a_person_who_stopped_the_bot_or_a_chat_that_is_not_found_is_the_end_of_the_route() {
        for words in [
            "Forbidden: bot was blocked by the user",
            "Forbidden: user is deactivated",
            "Forbidden: bot can't initiate conversation with a user",
        ] {
            let error = refusal("sendMessage", 403, &envelope(403, words, None));
            assert!(error.gone && !error.retryable, "{words}");
            assert!(error.message.contains(words));
        }
        assert!(
            refusal(
                "sendMessage",
                400,
                &envelope(400, "Bad Request: chat not found", None)
            )
            .gone
        );
    }

    #[test]
    fn flood_control_names_the_time_to_wait_and_is_bounded() {
        let error = refusal(
            "sendMessage",
            429,
            &envelope(429, "Too Many Requests: retry after 5", Some(5)),
        );
        assert_eq!((error.retry_after_secs, error.retryable), (Some(5), true));
        let parked = refusal(
            "sendMessage",
            429,
            &envelope(429, "Too Many Requests", Some(4_000_000)),
        );
        assert_eq!(parked.retry_after_secs, Some(MAX_RETRY_AFTER_SECS));
        assert!(
            refusal(
                "sendMessage",
                429,
                &envelope(429, "Too Many Requests", None)
            )
            .retryable
        );
    }

    #[test]
    fn a_token_that_is_not_a_bots_and_a_text_that_is_wrong_fail_for_good_and_a_conflict_or_a_server_error_passes()
     {
        let wrong_token = refusal("getMe", 401, &envelope(401, "Unauthorized", None));
        assert_eq!(
            (
                wrong_token.code.as_str(),
                wrong_token.retryable,
                wrong_token.gone
            ),
            ("unauthorized", false, false)
        );
        assert_eq!(
            refusal("getMe", 404, &envelope(404, "Not Found", None)).code,
            "unauthorized"
        );
        let too_long = refusal(
            "sendMessage",
            400,
            &envelope(400, "Bad Request: message is too long", None),
        );
        assert!(!too_long.retryable && !too_long.gone);
        let conflict = refusal(
            "getUpdates",
            409,
            &envelope(
                409,
                "Conflict: terminated by other getUpdates request",
                None,
            ),
        );
        assert!(
            conflict.retryable,
            "a reading that a worker that is gone left waiting ends by itself"
        );
        assert!(refusal("sendMessage", 502, &envelope(502, "Bad Gateway", None)).retryable);
        // A 403 on a reading is not the end of anyone's route.
        assert!(!refusal("getUpdates", 403, &envelope(403, "Forbidden", None)).gone);
    }

    #[test]
    fn a_person_is_named_by_their_name_or_else_their_username() {
        let user = |first: &str, last: Option<&str>, username: Option<&str>| User {
            id: 42,
            first_name: first.to_owned(),
            last_name: last.map(str::to_owned),
            username: username.map(str::to_owned),
        };
        assert_eq!(
            user("Dana", Some("Scully"), Some("dana")).label(),
            "Dana Scully"
        );
        assert_eq!(user("Dana", None, None).label(), "Dana");
        assert_eq!(user("", None, Some("dana")).label(), "@dana");
        assert_eq!(user("", None, None).label(), "42");
    }
}

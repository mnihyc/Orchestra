//! Masking of what must not be stored in a chat: the values of the secrets a node resolves, and the patterns an org asks to be masked.
//!
//! Text is masked when it is written, before it is stored and before anybody
//! is told of it, so that nothing unmasked is ever read back. The patterns are
//! those `redaction.patterns` resolves to at the chat's node (at its root org
//! for a direct or a group chat); in the chat of a topic or of a thread the
//! values of that topic's secrets are masked as well, unless
//! `redaction.mask_secrets` is off there.

use crate::Error;
use orch_api_contract::chats::ChatPart;
use orch_db::Conn;
use orch_db::queries::chats::{self as rows, ChatRow};
use orch_db::secrets::MasterKey;
use orch_settings::Place;
use orch_settings::secrets::{self, Owner};
use regex::Regex;
use serde::Deserialize;

/// What a masked value is replaced by.
pub const MASK: &str = "[masked]";

/// The fewest characters a secret has to have to be masked wherever it
/// occurs: a shorter value would turn words that happen to contain it into
/// masks, and a value that short is no secret.
pub const MIN_SECRET_CHARS: usize = 4;

/// How a deployment hands over the key that opens secrets: asked for only when
/// the node has secrets, and failing, rather than leaving text unmasked, when
/// it cannot be read.
pub type KeyProvider<'a> = &'a (dyn Fn() -> Result<&'static MasterKey, String> + Sync);

/// A pattern to mask, as `redaction.patterns` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Pattern {
    /// What it is called.
    pub name: String,
    /// The regular expression.
    pub regex: String,
}

/// What masks text.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    patterns: Vec<Regex>,
    secrets: Vec<String>,
}

impl Redactor {
    /// A redactor that masks nothing.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// A redactor for these patterns and these secret values. A pattern that
    /// is no regular expression is left out and told of in the log, since a
    /// setting that cannot be used should not stop people from talking; a
    /// secret too short to mask is left out.
    #[must_use]
    pub fn new(patterns: &[Pattern], secrets: Vec<String>) -> Self {
        let patterns = patterns
            .iter()
            .filter_map(|pattern| match Regex::new(&pattern.regex) {
                Ok(regex) => Some(regex),
                Err(error) => {
                    tracing::warn!(name = %pattern.name, %error, "a redaction pattern is not a regular expression");
                    None
                }
            })
            .collect();
        let mut secrets: Vec<String> = secrets
            .into_iter()
            .filter(|secret| secret.chars().count() >= MIN_SECRET_CHARS)
            .collect();
        // The longest first, so that a value that holds another is masked whole.
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        secrets.dedup();
        Self { patterns, secrets }
    }

    /// The text with every secret value and every match of a pattern masked.
    #[must_use]
    pub fn mask(&self, text: &str) -> String {
        let mut masked = text.to_owned();
        for secret in &self.secrets {
            masked = masked.replace(secret.as_str(), MASK);
        }
        for pattern in &self.patterns {
            masked = pattern.replace_all(&masked, MASK).into_owned();
        }
        masked
    }

    /// Masks the text of each part that has some.
    pub fn mask_parts(&self, parts: &mut [ChatPart]) {
        for part in parts {
            if let ChatPart::Text { text } = part {
                *text = self.mask(text);
            }
        }
    }
}

/// The redactor for what is written in `chat`: the patterns that
/// `redaction.patterns` resolves to, and the values of the topic's secrets in
/// the chat of a topic or of a thread.
pub async fn for_chat(
    conn: &mut Conn,
    chat: &ChatRow,
    keys: KeyProvider<'_>,
) -> Result<Redactor, Error> {
    let place = Place::Node(chat.node_id.unwrap_or(chat.org_id));
    let patterns: Vec<Pattern> = orch_settings::get(conn, place, "redaction.patterns")
        .await?
        .unwrap_or_default();
    let mask_secrets: bool = orch_settings::get(conn, place, "redaction.mask_secrets")
        .await?
        .unwrap_or(true);
    let mut values = Vec::new();
    if mask_secrets
        && let Some(node) = chat.node_id
        && let Some(topic) = rows::topic_of(conn, node).await?
    {
        let named = secrets::list(conn, Owner::Node(topic))
            .await
            .map_err(|error| Error::Unmasked(error.to_string()))?;
        if !named.is_empty() {
            let key = keys().map_err(Error::Unmasked)?;
            values = secrets::open_resolved(conn, key, Owner::Node(topic))
                .await
                .map_err(|error| Error::Unmasked(error.to_string()))?
                .into_iter()
                .map(|opened| opened.value)
                .collect();
        }
    }
    Ok(Redactor::new(&patterns, values))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(regex: &str) -> Pattern {
        Pattern {
            name: "token".to_owned(),
            regex: regex.to_owned(),
        }
    }

    #[test]
    fn a_secret_is_masked_wherever_it_occurs_and_the_longer_value_first() {
        let redactor = Redactor::new(&[], vec!["hunter2".to_owned(), "hunter2-extra".to_owned()]);
        assert_eq!(
            redactor.mask("the password is hunter2-extra, or hunter2"),
            format!("the password is {MASK}, or {MASK}")
        );
    }

    #[test]
    fn a_pattern_masks_what_it_matches_and_a_broken_pattern_masks_nothing() {
        let redactor = Redactor::new(&[pattern(r"sk-[a-z0-9]{8,}"), pattern("(")], Vec::new());
        assert_eq!(
            redactor.mask("use sk-abcdef123456 now"),
            format!("use {MASK} now")
        );
        assert_eq!(redactor.mask("nothing here"), "nothing here");
    }

    #[test]
    fn a_value_too_short_to_be_a_secret_is_not_masked() {
        let redactor = Redactor::new(&[], vec!["ab".to_owned()]);
        assert_eq!(redactor.mask("a table of abacus"), "a table of abacus");
    }

    #[test]
    fn only_text_parts_are_masked() {
        let redactor = Redactor::new(&[], vec!["hunter2".to_owned()]);
        let mut parts = vec![
            ChatPart::Text {
                text: "hunter2".to_owned(),
            },
            ChatPart::ItemRef {
                thread_id: orch_types::new_id(),
                item_id: orch_types::new_id(),
            },
        ];
        redactor.mask_parts(&mut parts);
        assert_eq!(
            parts[0],
            ChatPart::Text {
                text: MASK.to_owned()
            }
        );
    }
}

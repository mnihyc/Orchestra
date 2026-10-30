//! A value that is given to an implementation and never printed.
//!
//! A key, a token, a password or an enrollment token crosses an extension
//! point as a [`Secret`]. It is written as the text it holds, because the
//! implementation has to use it, and it is shown as nothing, so that a
//! structure that holds one can be logged without giving it away.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What stands in place of a secret wherever one is shown.
pub const REDACTED: &str = "[secret]";

/// A text that is used and never shown.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// A secret that holds `value`.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value, for the one place that uses it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The value, given up for good.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }

    /// Whether it holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(REDACTED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_is_written_as_its_text_and_shown_as_nothing() {
        let secret = Secret::new("sk-live-123");
        assert_eq!(serde_json::to_string(&secret).unwrap(), "\"sk-live-123\"");
        assert_eq!(format!("{secret:?}"), REDACTED);
        assert_eq!(format!("{:?}", Some(&secret)), format!("Some({REDACTED})"));
        let read: Secret = serde_json::from_str("\"sk-live-123\"").unwrap();
        assert_eq!(read.expose(), "sk-live-123");
    }
}

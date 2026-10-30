//! Forms issuable from an API credential.
//!
//! A provider's API key stays in the provider's runtime. A harness that must run elsewhere, to stay beside
//! its own tools, and that the forwarder cannot serve, can still run on a credential issued from the key:
//! the key as it is, or, for an upstream that can mint narrower or expiring keys, a copy of that kind. What a
//! wire protocol can issue is declared as data ([`crate::protocol::WireProtocol::credential_forms`]), and
//! every protocol has the form `api_key`: the key as it is, a copy that lasts. The forms are the same
//! declarations an adapter makes of what it issues from its login (`orch_agent_contract::credentials`);
//! a form that is on both sides' lists, the protocol's and the adapter's, can be used.

use orch_agent_contract::credentials::{API_KEY_FORM, CredentialForm, IssuedCredential};
use orch_plugin_contract::Secret;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The key of an API upstream. It is used and never shown; written as the text it holds, because an
/// external translator is given it in one request over the plugin's local channel.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct Credential(Secret);

impl Credential {
    /// A credential that holds `key`.
    #[must_use]
    pub fn new(key: impl Into<Secret>) -> Self {
        Self(key.into())
    }

    /// The key, for the one place that presents it.
    #[must_use]
    pub fn expose(&self) -> &str {
        self.0.expose()
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Credential({:?})", self.0)
    }
}

/// Why a credential could not be issued from a key.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    /// The protocol does not issue that form.
    #[error("this protocol does not issue the form {0:?}")]
    UnknownForm(String),
    /// The upstream refused, or answered something that cannot be used.
    #[error("the upstream answered {status}: {message}")]
    Upstream {
        /// The HTTP status it answered.
        status: u16,
        /// What it said.
        message: String,
    },
    /// The upstream could not be reached.
    #[error("the upstream could not be reached: {0}")]
    Unreachable(String),
    /// The implementation failed in a way that is its own.
    #[error("internal error: {0}")]
    Internal(String),
}

/// The forms every protocol has: the key as it is, and nothing else.
#[must_use]
pub fn standard_forms() -> Vec<CredentialForm> {
    vec![CredentialForm::api_key()]
}

/// The credential issued as the form `form` from `key`, for the forms that need no call to the upstream: the
/// form `api_key` is the key as it is. Any other form is for the protocol to issue.
pub fn issue_standard(form: &str, key: &Credential) -> Result<IssuedCredential, WireError> {
    if form == API_KEY_FORM {
        Ok(IssuedCredential {
            form: API_KEY_FORM.to_owned(),
            credential: Secret::new(key.expose()),
            expires_at: None,
        })
    } else {
        Err(WireError::UnknownForm(form.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_as_it_is_is_the_one_form_every_protocol_has() {
        let forms = standard_forms();
        assert_eq!(forms.len(), 1);
        assert_eq!(forms[0].form, "api_key");
        assert!(!forms[0].lapses(), "a key lasts until its owner rotates it");
        let issued = issue_standard("api_key", &Credential::new("sk-123")).unwrap();
        assert_eq!(issued.credential.expose(), "sk-123");
        assert_eq!(issued.expires_at, None);
    }

    #[test]
    fn a_form_the_protocol_does_not_have_is_refused_and_a_key_is_never_printed() {
        assert_eq!(
            issue_standard("short_lived", &Credential::new("sk-123")),
            Err(WireError::UnknownForm("short_lived".into()))
        );
        assert!(!format!("{:?}", Credential::new("sk-123")).contains("sk-123"));
    }
}

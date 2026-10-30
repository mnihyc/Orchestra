//! API-key credential form.
//!
//! The vendor's API takes one kind of credential, the key itself, and has no call that mints a narrower or an
//! expiring copy of it. The only form the provider role can issue from a key is therefore `api_key`: the key
//! as it is, a copy that lasts until the key is rotated or revoked. A harness that must run away from the
//! provider's runtime and accepts that form can run on it; any other form is refused.

use orch_agent_contract::credentials::{CredentialForm, IssuedCredential};
use orch_model_contract::credentials::{Credential, WireError, issue_standard, standard_forms};

/// The forms that can be issued from a key: the key as it is.
#[must_use]
pub fn forms() -> Vec<CredentialForm> {
    standard_forms()
}

/// Issues `key` as the credential `form`.
pub fn issue(form: &str, key: &Credential) -> Result<IssuedCredential, WireError> {
    issue_standard(form, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_as_it_is_is_the_only_form_and_it_lasts() {
        let forms = forms();
        assert_eq!(forms.len(), 1);
        assert_eq!(forms[0].form, "api_key");
        assert!(!forms[0].lapses());
        let issued = issue("api_key", &Credential::new("sk-1")).unwrap();
        assert_eq!(issued.credential.expose(), "sk-1");
        assert_eq!(issued.expires_at, None);
    }

    #[test]
    fn a_form_the_vendor_cannot_mint_is_refused() {
        assert_eq!(
            issue("access_token", &Credential::new("sk-1")),
            Err(WireError::UnknownForm("access_token".into()))
        );
    }
}

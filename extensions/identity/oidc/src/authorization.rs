//! State, nonce, PKCE and sign-in redirects.
//!
//! Every sign-in has three secrets of its own ([`PendingSignIn`]), each [`SECRET_BYTES`] random bytes
//! written in base64url: the state, which the browser brings back and which names the sign-in; the
//! nonce, which the provider writes into the one ID token it issues for this sign-in; and the PKCE
//! verifier, whose SHA-256 is sent at the start (`code_challenge`, method `S256`) while the verifier
//! itself goes only with the code, from this server to the provider, so that a code seen on its way
//! back through the browser is of no use to whoever saw it. A fresh proof for one sensitive act asks
//! the provider to sign the person in again now (`prompt=login`, `max_age=0`), so that the ID token
//! says when it did (`auth_time`).

use crate::discovery::Metadata;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_identity_contract::provider::{PendingSignIn, ProviderError, SignInStart, StartRequest};
use ring::digest::{SHA256, digest};
use ring::rand::{SecureRandom, SystemRandom};
use url::Url;

/// The random bytes of each secret of a sign-in: 256 bits, which nobody guesses, and 43 characters in
/// base64url, the shortest verifier PKCE allows.
pub const SECRET_BYTES: usize = 32;
/// The PKCE method: the SHA-256 of the verifier, never the verifier itself.
pub const PKCE_METHOD: &str = "S256";
/// What the start asks the provider for: a code, to be exchanged by this server.
pub const RESPONSE_TYPE: &str = "code";
/// What a fresh proof asks of the provider: to sign the person in again whatever session it has.
pub const PROMPT_LOGIN: &str = "login";
/// The oldest sign-in at the provider a fresh proof takes: none older than now.
pub const MAX_AGE_NOW: &str = "0";

/// A new secret: [`SECRET_BYTES`] random bytes from the operating system, in base64url.
pub fn new_secret() -> Result<String, ProviderError> {
    let mut bytes = [0_u8; SECRET_BYTES];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| ProviderError::new("the operating system gave no random bytes"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// The three secrets of a new sign-in, each its own.
pub fn new_pending() -> Result<PendingSignIn, ProviderError> {
    Ok(PendingSignIn {
        state: new_secret()?,
        nonce: new_secret()?,
        pkce_verifier: new_secret()?,
    })
}

/// What the provider is told of a verifier at the start: its SHA-256 in base64url.
#[must_use]
pub fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(digest(&SHA256, verifier.as_bytes()))
}

/// The address the browser is sent to: the provider's authorization endpoint, with what it already
/// carries in its query kept, and the request of this sign-in after it.
#[must_use]
pub fn redirect_url(endpoint: &Url, request: &StartRequest, pending: &PendingSignIn) -> String {
    let mut url = endpoint.clone();
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", RESPONSE_TYPE)
            .append_pair("client_id", &request.settings.client_id)
            .append_pair("redirect_uri", &request.callback_url)
            .append_pair("scope", &request.settings.scopes.join(" "))
            .append_pair("state", &pending.state)
            .append_pair("nonce", &pending.nonce)
            .append_pair("code_challenge", &challenge(&pending.pkce_verifier))
            .append_pair("code_challenge_method", PKCE_METHOD);
        if request.fresh {
            query
                .append_pair("prompt", PROMPT_LOGIN)
                .append_pair("max_age", MAX_AGE_NOW);
        }
    }
    url.into()
}

/// Begins a sign-in with the provider described by `metadata`.
pub fn start(metadata: &Metadata, request: &StartRequest) -> Result<SignInStart, ProviderError> {
    let pending = new_pending()?;
    Ok(SignInStart {
        redirect_url: redirect_url(&metadata.authorization_endpoint, request, &pending),
        pending,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_identity_contract::provider::ProviderSettings;
    use serde_json::json;
    use std::collections::HashMap;

    fn request(fresh: bool) -> StartRequest {
        let settings: ProviderSettings = serde_json::from_value(json!({
            "key": "acme", "name": "Acme", "issuer": "https://login.acme.example", "client_id": "orch"
        }))
        .unwrap();
        StartRequest {
            settings,
            callback_url: "https://orch.example.com/api/v1/auth/oidc/acme/callback".to_owned(),
            fresh,
        }
    }

    fn query_of(address: &str) -> HashMap<String, String> {
        Url::parse(address)
            .unwrap()
            .query_pairs()
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect()
    }

    #[test]
    fn a_challenge_is_the_sha256_of_the_verifier_in_base64url() {
        // The example of RFC 7636, appendix B.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn every_sign_in_has_secrets_of_its_own_that_the_address_carries_but_the_verifier() {
        let endpoint = Url::parse("https://login.acme.example/authorize?tenant=main").unwrap();
        let first = new_pending().unwrap();
        let second = new_pending().unwrap();
        assert_ne!(first.state, second.state);
        assert_ne!(first.nonce, second.nonce);
        assert_ne!(first.pkce_verifier, second.pkce_verifier);
        assert_eq!(first.pkce_verifier.len(), 43);
        let query = query_of(&redirect_url(&endpoint, &request(false), &first));
        assert_eq!(query["tenant"], "main", "what the endpoint carries is kept");
        assert_eq!(query["response_type"], "code");
        assert_eq!(query["client_id"], "orch");
        assert_eq!(query["scope"], "openid profile email");
        assert_eq!(query["state"], first.state);
        assert_eq!(query["nonce"], first.nonce);
        assert_eq!(query["code_challenge"], challenge(&first.pkce_verifier));
        assert_eq!(query["code_challenge_method"], "S256");
        assert!(
            query.values().all(|value| value != &first.pkce_verifier),
            "the verifier never leaves this server through the browser"
        );
        assert!(!query.contains_key("prompt") && !query.contains_key("max_age"));
    }

    #[test]
    fn a_fresh_proof_asks_the_provider_to_sign_the_person_in_again_now() {
        let endpoint = Url::parse("https://login.acme.example/authorize").unwrap();
        let query = query_of(&redirect_url(
            &endpoint,
            &request(true),
            &new_pending().unwrap(),
        ));
        assert_eq!(query["prompt"], "login");
        assert_eq!(query["max_age"], "0");
    }
}

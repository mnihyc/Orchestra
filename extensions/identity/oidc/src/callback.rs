//! Verified identity claims and callback outcomes.
//!
//! The code the browser brought is exchanged at the provider's token endpoint, from this server, with
//! the PKCE verifier of the sign-in and the client's credentials (HTTP Basic as RFC 6749 writes it,
//! or in the form where the provider takes only that, or the client id alone for a client without a
//! secret). The ID token that comes back is believed only after every check of OpenID Connect's core:
//!
//! - its signature, by a key the issuer publishes, of an algorithm that is taken and that the issuer
//!   says it signs with; a key it does not know makes the published keys be read again once;
//! - its issuer, which is exactly the one the issuer's description names;
//! - its audience, which is this client and no other, and its authorized party where it names one;
//! - its times: not ended (`exp`), not issued in the future nor before the sign-in it answers could have
//!   begun (`iat`), not yet to begin (`nbf`), each with [`LEEWAY_SECS`] for the clocks;
//! - its nonce, which is the one this sign-in made, compared in constant time.
//!
//! The identity is then read from its claims under the names the provider is set to use. Where the
//! token leaves out the address, the name or the username and the provider has a user information
//! endpoint, that is asked with the access token, and what it says is taken only where it speaks of the
//! same subject; the address and whether it is confirmed are read from one place together.

use crate::discovery::{Cache, Metadata};
use crate::jws::{Jwk, Signed};
use crate::{JSON_TYPE, get_json, read_bounded};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use orch_identity_contract::claims::{ClaimNames, EMAIL_VERIFIED_CLAIM, VerifiedIdentity};
use orch_identity_contract::provider::{CompleteRequest, ProviderError, SIGN_IN_LIFETIME_SECS};
use orch_types::Json;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde_json::Map;
use subtle::ConstantTimeEq;
use url::form_urlencoded;

/// How far the provider's clock and this system's may differ, in seconds: a minute, what most clients
/// allow; more would keep an ended token good for longer.
pub const LEEWAY_SECS: i64 = 60;
/// The media type of the token request.
const FORM_TYPE: &str = "application/x-www-form-urlencoded";
/// The type of access token that may be shown to the user information endpoint.
const BEARER: &str = "bearer";
/// How a client sends its secret where the provider names no way.
const CLIENT_SECRET_BASIC: &str = "client_secret_basic";
/// How a client sends its secret in the form of the request.
const CLIENT_SECRET_POST: &str = "client_secret_post";

/// What the token endpoint gave.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Tokens {
    /// The ID token.
    id_token: String,
    /// The access token, where it is a bearer token.
    access_token: Option<String>,
}

/// Ends a sign-in: exchanges the code, checks the ID token, and reads the identity.
pub async fn complete(
    client: &reqwest::Client,
    cache: &Cache,
    request: &CompleteRequest,
) -> Result<VerifiedIdentity, ProviderError> {
    let issuer = &request.settings.issuer;
    let metadata = cache.metadata(client, issuer).await?;
    let tokens = exchange(client, &metadata, request).await?;
    let token = Signed::read(&tokens.id_token)?;
    if !metadata.allows(token.alg) {
        return Err(ProviderError::new(format!(
            "the ID token is signed with {}, which the provider does not say it signs with",
            token.alg.name()
        )));
    }
    let key = signing_key(client, cache, issuer, &metadata, &token).await?;
    token.verify(&key)?;
    check_claims(&token.claims, &metadata.issuer, request)?;
    let claims = with_user_info(
        client,
        &metadata,
        tokens.access_token.as_deref(),
        token.claims,
        &request.settings.claims,
    )
    .await;
    VerifiedIdentity::from_claims(&request.settings.claims, &Json::Object(claims))
        .map_err(|error| ProviderError::new(format!("the ID token names nobody: {error}")))
}

/// Whether the client sends its secret in the form: where the provider takes that and not HTTP Basic.
fn posts_secret(metadata: &Metadata) -> bool {
    let offers = |method: &str| metadata.auth_methods.iter().any(|each| each == method);
    !metadata.auth_methods.is_empty() && !offers(CLIENT_SECRET_BASIC) && offers(CLIENT_SECRET_POST)
}

/// The `Authorization` header of HTTP Basic for a client: the id and the secret are each written as a
/// form writes a value before they are joined, as RFC 6749 (2.3.1) asks.
fn basic(client_id: &str, secret: &str) -> String {
    let encode = |text: &str| form_urlencoded::byte_serialize(text.as_bytes()).collect::<String>();
    format!(
        "Basic {}",
        STANDARD.encode(format!("{}:{}", encode(client_id), encode(secret)))
    )
}

/// Exchanges the code for the tokens.
async fn exchange(
    client: &reqwest::Client,
    metadata: &Metadata,
    request: &CompleteRequest,
) -> Result<Tokens, ProviderError> {
    let client_id = request.settings.client_id.as_str();
    let secret = request
        .client_secret
        .as_ref()
        .map(|secret| secret.expose())
        .filter(|secret| !secret.is_empty());
    // The form is written out in a block of its own: its writer is not `Send`, and the future of a
    // provider kind has to be, so the writer ends before anything is awaited.
    let (form, authorization) = {
        let mut form = form_urlencoded::Serializer::new(String::new());
        form.append_pair("grant_type", "authorization_code")
            .append_pair("code", &request.code)
            .append_pair("redirect_uri", &request.callback_url)
            .append_pair("code_verifier", &request.pending.pkce_verifier);
        let authorization = match secret {
            None => {
                form.append_pair("client_id", client_id);
                None
            }
            Some(secret) if posts_secret(metadata) => {
                form.append_pair("client_id", client_id)
                    .append_pair("client_secret", secret);
                None
            }
            Some(secret) => Some(basic(client_id, secret)),
        };
        (form.finish(), authorization)
    };
    let mut post = client
        .post(metadata.token_endpoint.clone())
        .header(ACCEPT, JSON_TYPE)
        .header(CONTENT_TYPE, FORM_TYPE);
    if let Some(authorization) = authorization {
        post = post.header(AUTHORIZATION, authorization);
    }
    let response = post.body(form).send().await.map_err(|error| {
        ProviderError::new(format!("the token endpoint could not be reached: {error}"))
    })?;
    let status = response.status();
    let body = read_bounded(response).await?;
    let answer: Option<Json> = serde_json::from_slice(&body).ok();
    if !status.is_success() {
        let said = |name: &str| {
            answer
                .as_ref()
                .and_then(|answer| answer.get(name))
                .and_then(Json::as_str)
                .map(str::to_owned)
        };
        let error = said("error").unwrap_or_else(|| "no error code".to_owned());
        let description = said("error_description")
            .map(|text| format!(": {text}"))
            .unwrap_or_default();
        return Err(ProviderError::new(format!(
            "the token endpoint refused the code ({status}, {error}{description})"
        )));
    }
    let answer =
        answer.ok_or_else(|| ProviderError::new("the token endpoint did not answer with JSON"))?;
    let id_token = answer
        .get("id_token")
        .and_then(Json::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            ProviderError::new("the token endpoint gave no ID token: is openid among the scopes?")
        })?;
    let bearer = answer
        .get("token_type")
        .and_then(Json::as_str)
        .is_some_and(|kind| kind.eq_ignore_ascii_case(BEARER));
    let access_token = answer
        .get("access_token")
        .and_then(Json::as_str)
        .filter(|token| bearer && !token.is_empty())
        .map(str::to_owned);
    Ok(Tokens {
        id_token: id_token.to_owned(),
        access_token,
    })
}

/// The published key the token names: from the keys kept, or, where they do not have it, from the
/// keys read again now, once.
async fn signing_key(
    client: &reqwest::Client,
    cache: &Cache,
    issuer: &str,
    metadata: &Metadata,
    token: &Signed,
) -> Result<Jwk, ProviderError> {
    let (keys, read_now) = cache.keys(client, issuer, metadata, false).await?;
    if let Some(key) = keys.find(token.kid.as_deref(), token.alg) {
        return Ok(key.clone());
    }
    if !read_now {
        // A key the kept set does not have: the provider may have rotated its keys since.
        let (keys, _) = cache.keys(client, issuer, metadata, true).await?;
        if let Some(key) = keys.find(token.kid.as_deref(), token.alg) {
            return Ok(key.clone());
        }
    }
    Err(ProviderError::new(match &token.kid {
        Some(kid) => {
            format!("the ID token is signed with a key ({kid}) the provider does not publish")
        }
        None => {
            "the ID token names no key, and the provider publishes no single key it fits".to_owned()
        }
    }))
}

/// A time claim in whole seconds since the epoch, where it is a number.
fn seconds(claims: &Map<String, Json>, name: &str) -> Option<i64> {
    let value = claims.get(name)?;
    value.as_i64().or_else(|| {
        value
            .as_f64()
            .filter(|seconds| seconds.is_finite())
            .map(|seconds| seconds.floor() as i64)
    })
}

/// Checks what makes the claims an answer to this sign-in, for this client, now (see the module).
fn check_claims(
    claims: &Map<String, Json>,
    issuer: &str,
    request: &CompleteRequest,
) -> Result<(), ProviderError> {
    match claims.get("iss").and_then(Json::as_str) {
        Some(named) if named == issuer => {}
        Some(named) => {
            return Err(ProviderError::new(format!(
                "the ID token was issued by {named}, not by {issuer}"
            )));
        }
        None => return Err(ProviderError::new("the ID token names no issuer")),
    }
    if !claims
        .get("sub")
        .and_then(Json::as_str)
        .is_some_and(|subject| !subject.is_empty())
    {
        return Err(ProviderError::new("the ID token names no subject"));
    }
    let client_id = request.settings.client_id.as_str();
    let audiences: Option<Vec<&str>> = match claims.get("aud") {
        Some(Json::String(one)) => Some(vec![one.as_str()]),
        Some(Json::Array(many)) => many.iter().map(Json::as_str).collect(),
        _ => None,
    };
    let audiences = audiences.unwrap_or_default();
    if !audiences.contains(&client_id) {
        return Err(ProviderError::new(format!(
            "the ID token is not for the client {client_id}"
        )));
    }
    if audiences.iter().any(|audience| *audience != client_id) {
        return Err(ProviderError::new(
            "the ID token is for other clients too, which this one does not trust",
        ));
    }
    if let Some(party) = claims.get("azp")
        && party.as_str() != Some(client_id)
    {
        return Err(ProviderError::new(
            "the ID token was given to another client (azp)",
        ));
    }
    let now = request.now.get().timestamp();
    let ends = seconds(claims, "exp")
        .ok_or_else(|| ProviderError::new("the ID token has no end (exp)"))?;
    if now >= ends.saturating_add(LEEWAY_SECS) {
        return Err(ProviderError::new("the ID token has ended (exp)"));
    }
    let issued = seconds(claims, "iat")
        .ok_or_else(|| ProviderError::new("the ID token says not when it was issued (iat)"))?;
    if issued > now.saturating_add(LEEWAY_SECS) {
        return Err(ProviderError::new(
            "the ID token says it was issued in the future (iat)",
        ));
    }
    if issued < now.saturating_sub(SIGN_IN_LIFETIME_SECS.saturating_add(LEEWAY_SECS)) {
        return Err(ProviderError::new(
            "the ID token was issued before this sign-in could have begun (iat)",
        ));
    }
    if seconds(claims, "nbf").is_some_and(|begins| begins > now.saturating_add(LEEWAY_SECS)) {
        return Err(ProviderError::new("the ID token is not good yet (nbf)"));
    }
    let answers = claims
        .get("nonce")
        .and_then(Json::as_str)
        .is_some_and(|nonce| bool::from(nonce.as_bytes().ct_eq(request.pending.nonce.as_bytes())));
    if !answers {
        return Err(ProviderError::new(
            "the ID token does not answer this sign-in (its nonce is another)",
        ));
    }
    Ok(())
}

/// The claims of the ID token, with what the user information endpoint tells of the same subject
/// where the token leaves out the address, the name or the username. A provider whose endpoint fails
/// leaves the token's claims as they are: the token alone is the identity.
async fn with_user_info(
    client: &reqwest::Client,
    metadata: &Metadata,
    access_token: Option<&str>,
    mut claims: Map<String, Json>,
    names: &ClaimNames,
) -> Map<String, Json> {
    let wanted = [
        names.email.as_str(),
        names.name.as_str(),
        names.username.as_str(),
    ];
    if wanted.iter().all(|name| claims.contains_key(*name)) {
        return claims;
    }
    let (Some(endpoint), Some(access_token)) = (&metadata.userinfo_endpoint, access_token) else {
        return claims;
    };
    let Ok(Json::Object(info)) = get_json(client, endpoint.as_str(), Some(access_token)).await
    else {
        return claims;
    };
    // What it says is of the same person, or of nobody.
    if info.get("sub").and_then(Json::as_str) != claims.get("sub").and_then(Json::as_str) {
        return claims;
    }
    // The address and whether it is confirmed come from one place together.
    let address_from_info = !claims.contains_key(&names.email) && info.contains_key(&names.email);
    for name in wanted {
        if !claims.contains_key(name)
            && let Some(value) = info.get(name)
        {
            claims.insert(name.to_owned(), value.clone());
        }
    }
    if address_from_info {
        match info.get(EMAIL_VERIFIED_CLAIM) {
            Some(verified) => {
                claims.insert(EMAIL_VERIFIED_CLAIM.to_owned(), verified.clone());
            }
            None => {
                claims.remove(EMAIL_VERIFIED_CLAIM);
            }
        }
    }
    claims
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_identity_contract::provider::{PendingSignIn, ProviderSettings};
    use serde_json::json;

    fn request(now: i64) -> CompleteRequest {
        let settings: ProviderSettings = serde_json::from_value(json!({
            "key": "acme", "name": "Acme", "issuer": "https://login.acme.example", "client_id": "orch"
        }))
        .unwrap();
        CompleteRequest {
            settings,
            client_secret: None,
            callback_url: "https://orch.example.com/api/v1/auth/oidc/acme/callback".to_owned(),
            pending: PendingSignIn {
                state: "s".to_owned(),
                nonce: "n-1".to_owned(),
                pkce_verifier: "v".to_owned(),
            },
            code: "c".to_owned(),
            now: orch_types::Time::new(chrono::DateTime::from_timestamp(now, 0).unwrap()),
        }
    }

    const NOW: i64 = 1_791_000_000;

    fn good() -> Map<String, Json> {
        let Json::Object(claims) = json!({
            "iss": "https://login.acme.example", "sub": "248289761001", "aud": "orch",
            "iat": NOW - 5, "exp": NOW + 300, "nonce": "n-1"
        }) else {
            unreachable!()
        };
        claims
    }

    fn with(change: &[(&str, Json)]) -> Map<String, Json> {
        let mut claims = good();
        for (name, value) in change {
            if value.is_null() {
                claims.remove(*name);
            } else {
                claims.insert((*name).to_owned(), value.clone());
            }
        }
        claims
    }

    fn check(claims: &Map<String, Json>) -> Result<(), ProviderError> {
        check_claims(claims, "https://login.acme.example", &request(NOW))
    }

    #[test]
    fn an_answer_to_this_sign_in_for_this_client_now_passes() {
        assert_eq!(check(&good()), Ok(()));
        assert_eq!(
            check(&with(&[("aud", json!(["orch"])), ("azp", json!("orch"))])),
            Ok(())
        );
        assert_eq!(
            check(&with(&[("exp", json!(NOW - 30))])),
            Ok(()),
            "within the leeway for the clocks"
        );
        assert_eq!(check(&with(&[("iat", json!(NOW as f64 + 0.5))])), Ok(()));
    }

    #[test]
    fn another_issuer_audience_party_nonce_or_a_time_out_of_bounds_is_refused() {
        for (change, why) in [
            (vec![("iss", json!("https://evil.example"))], "issuer"),
            (vec![("iss", Json::Null)], "no issuer"),
            (vec![("sub", json!(""))], "no subject"),
            (vec![("aud", json!("someone-else"))], "audience"),
            (
                vec![("aud", json!(["orch", "someone-else"]))],
                "another audience beside",
            ),
            (vec![("aud", json!([1]))], "an audience that is no text"),
            (vec![("azp", json!("someone-else"))], "authorized party"),
            (vec![("exp", json!(NOW - 61))], "ended"),
            (vec![("exp", Json::Null)], "no end"),
            (vec![("iat", json!(NOW + 61))], "issued in the future"),
            (
                vec![("iat", json!(NOW - 661))],
                "issued before the sign-in began",
            ),
            (vec![("iat", Json::Null)], "no issue time"),
            (vec![("nbf", json!(NOW + 120))], "not good yet"),
            (vec![("nonce", json!("n-2"))], "another nonce"),
            (vec![("nonce", Json::Null)], "no nonce"),
        ] {
            assert!(check(&with(&change)).is_err(), "{why}");
        }
    }

    #[test]
    fn a_client_secret_goes_by_basic_unless_the_provider_takes_it_only_in_the_form() {
        assert_eq!(
            basic("my client", "s3cret:é"),
            format!("Basic {}", STANDARD.encode("my+client:s3cret%3A%C3%A9"))
        );
        let described = |methods: Json| {
            let mut document = json!({
                "issuer": "https://login.acme.example",
                "authorization_endpoint": "https://login.acme.example/auth",
                "token_endpoint": "https://login.acme.example/token",
                "jwks_uri": "https://login.acme.example/keys"
            });
            if !methods.is_null() {
                document["token_endpoint_auth_methods_supported"] = methods;
            }
            Metadata::read("https://login.acme.example", document).unwrap()
        };
        assert!(!posts_secret(&described(Json::Null)));
        assert!(!posts_secret(&described(json!([
            "client_secret_basic",
            "client_secret_post"
        ]))));
        assert!(posts_secret(&described(json!(["client_secret_post"]))));
        assert!(!posts_secret(&described(json!(["private_key_jwt"]))));
    }
}

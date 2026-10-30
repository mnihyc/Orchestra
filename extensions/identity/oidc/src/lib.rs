//! OpenID Connect identity-provider implementation.
//!
//! The kind `oidc` ([`OidcKind`]) signs people in with the authorization code flow of OpenID Connect,
//! as a confidential client (or a public one where no secret is set), with PKCE (`S256`) and a nonce
//! on every sign-in. What it believes of a provider it reads from the provider itself:
//!
//! - [`discovery`] reads the issuer's description of itself and the keys it signs with, keeps both for
//!   a while, and reads the keys again at once when a token names one it does not know (a rotation);
//! - [`authorization`] makes the three secrets of a sign-in and the address the browser is sent to;
//! - [`callback`] exchanges the code for the tokens, checks the ID token (its signature against the
//!   published keys, its issuer, audience, times and nonce), and reads the identity from it, with what
//!   the provider tells of the person at its user information endpoint where the token leaves a claim
//!   out;
//! - [`jws`] reads a signed token and the published keys, and checks a signature.
//!
//! Nothing here reads the database, and no time is taken from the machine for a decision: the API
//! gives the provider's settings, the unsealed client secret and the moment of the completion in each
//! request. Every request to a provider is bounded in time and in the size of its answer, and follows
//! no redirect: a provider answers at the addresses it publishes.

#![forbid(unsafe_code)]

pub mod authorization;
pub mod callback;
pub mod discovery;
pub mod jws;

use async_trait::async_trait;
use orch_identity_contract::claims::VerifiedIdentity;
use orch_identity_contract::provider::{
    CompleteRequest, Discovered, IdentityProviderKind, KIND_OIDC, ProviderError, ProviderSettings,
    SignInStart, StartRequest,
};
use orch_types::Json;
use reqwest::header::ACCEPT;
use std::time::Duration;

/// How long one request to a provider may take in all: ten seconds, as long as a person waiting at a
/// callback is asked to wait for one step of it; a provider slower than that is treated as down.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a connection to a provider may take to open: a provider that is not there is said to be
/// so within a few seconds.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The largest answer read from a provider: a mebibyte, far more than a description, a key set or a
/// token answer holds, and a bound on what a broken provider can make this server hold.
pub const MAX_ANSWER_BYTES: usize = 1024 * 1024;
/// The media type of what is asked for.
pub const JSON_TYPE: &str = "application/json";

/// The kind of identity provider that speaks OpenID Connect.
#[derive(Debug)]
pub struct OidcKind {
    client: reqwest::Client,
    cache: discovery::Cache,
}

impl OidcKind {
    /// A kind with an HTTP client of its own, which follows no redirect and gives up after
    /// [`REQUEST_TIMEOUT`].
    pub fn new() -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| {
                ProviderError::new(format!("the HTTP client could not be made: {error}"))
            })?;
        Ok(Self {
            client,
            cache: discovery::Cache::default(),
        })
    }
}

#[async_trait]
impl IdentityProviderKind for OidcKind {
    fn kind(&self) -> &'static str {
        KIND_OIDC
    }

    async fn start(&self, request: &StartRequest) -> Result<SignInStart, ProviderError> {
        let metadata = self
            .cache
            .metadata(&self.client, &request.settings.issuer)
            .await?;
        authorization::start(&metadata, request)
    }

    async fn complete(&self, request: &CompleteRequest) -> Result<VerifiedIdentity, ProviderError> {
        callback::complete(&self.client, &self.cache, request).await
    }

    async fn validate(&self, settings: &ProviderSettings) -> Result<Discovered, ProviderError> {
        discovery::validate(&self.client, &self.cache, settings).await
    }
}

/// Reads an answer of at most [`MAX_ANSWER_BYTES`].
pub(crate) async fn read_bounded(
    mut response: reqwest::Response,
) -> Result<Vec<u8>, ProviderError> {
    let limit = u64::try_from(MAX_ANSWER_BYTES).unwrap_or(u64::MAX);
    let too_large = || {
        ProviderError::new(format!(
            "the answer is larger than {MAX_ANSWER_BYTES} bytes"
        ))
    };
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Err(too_large());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| ProviderError::new(format!("the answer could not be read: {error}")))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_ANSWER_BYTES {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Reads the JSON document at `url`, with `bearer` as the access token where one is given.
pub(crate) async fn get_json(
    client: &reqwest::Client,
    url: &str,
    bearer: Option<&str>,
) -> Result<Json, ProviderError> {
    let mut request = client.get(url).header(ACCEPT, JSON_TYPE);
    if let Some(token) = bearer {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .map_err(|error| ProviderError::new(format!("{url} could not be reached: {error}")))?;
    let status = response.status();
    let body = read_bounded(response).await?;
    if !status.is_success() {
        return Err(ProviderError::new(format!("{url} answered {status}")));
    }
    serde_json::from_slice(&body)
        .map_err(|_| ProviderError::new(format!("{url} did not answer with JSON")))
}

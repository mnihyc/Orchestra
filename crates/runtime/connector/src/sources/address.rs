//! Bounded provider source requests to one authorized address, without redirects, cookies or credential-bearing errors.

use super::reports::scrub_message;
use crate::upstream::{Upstream, UpstreamError, UpstreamRequest};
use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use http_body_util::{BodyExt, Limited};
use orch_link_contract::policy::LocalPolicy;
pub use orch_provider_contract::SourceError as AddressError;
use orch_provider_contract::{AddressAuth, SourceAddress, SourceErrorCode, address, limits, wire};
use orch_types::Json;
use std::time::Duration;

use async_trait::async_trait;
use orch_plugin_contract::Secret;
use orch_provider_contract::wire::ResetSpendParams;
use orch_provider_contract::{
    FactsReport, FactsSource, ModelsReport, ModelsSource, ResetReport, SourceAsk, SourceDescriptor,
    SourceError,
};
use orch_types::{Clock, Time};
use std::sync::{Arc, Mutex, PoisonError};

/// All source methods have the same reply bound, including an unexpected successful response.
const REPLY_BYTES: usize = limits::REPORT_MAX_BYTES;
/// A source request may not carry an unbounded configuration or secret collection either.
const REQUEST_BYTES: usize = REPLY_BYTES;

/// Opened authorization, kept in the credential's machine and never formatted for a log.
pub enum OpenAuthorization<'a> {
    /// The address requires no authorization of its own.
    None,
    /// The address's own bearer token, not a provider sign-in.
    Bearer(&'a str),
    /// A header token with the address's configured name.
    Header { name: &'a str, value: &'a str },
}

impl std::fmt::Debug for OpenAuthorization<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "None",
            Self::Bearer(_) => "Bearer([secret])",
            Self::Header { .. } => "Header([secret])",
        })
    }
}

/// The already-selected binding, with the server's scheme permission and the keeping machine's local policy.
pub struct AddressCall<'a> {
    /// Exactly this address is contacted; no redirect may receive its authorization.
    pub url: &'a str,
    /// Authorization opened for this call only.
    pub authorization: OpenAuthorization<'a>,
    /// A source method, such as facts.read.
    pub method: &'a str,
    /// SourceAsk or reset parameters; only the selected, explicitly permitted credential belongs here.
    pub params: &'a Json,
    /// The server checked providers.allow_http_upstreams for this binding.
    pub allow_http: bool,
    /// None on a managed runtime; a connected machine always supplies its own policy.
    pub local_policy: Option<&'a LocalPolicy>,
    /// Every secret available to this source call, to scrub a source's own error sentence.
    pub secrets: &'a [String],
}

fn error(code: &'static str, message: &str, retryable: bool, secrets: &[String]) -> AddressError {
    AddressError {
        code: code.parse().unwrap_or(SourceErrorCode::Malformed),
        message: scrub_message(message, secrets),
        retryable,
    }
}

fn headers(auth: &OpenAuthorization<'_>, secrets: &[String]) -> Result<HeaderMap, AddressError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(header::CONNECTION, HeaderValue::from_static("close"));
    headers.insert(
        header::USER_AGENT,
        HeaderValue::from_static(concat!("orch-connector/", env!("CARGO_PKG_VERSION"))),
    );
    let token = match auth {
        OpenAuthorization::None => return Ok(headers),
        OpenAuthorization::Bearer(token) => (header::AUTHORIZATION, format!("Bearer {token}")),
        OpenAuthorization::Header { name, value } => {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                error(
                    "malformed",
                    "the source's authorization header is invalid",
                    false,
                    secrets,
                )
            })?;
            (name, (*value).to_owned())
        }
    };
    let mut value = HeaderValue::from_str(&token.1).map_err(|_| {
        error(
            "malformed",
            "the source's authorization value is invalid",
            false,
            secrets,
        )
    })?;
    value.set_sensitive(true);
    headers.insert(token.0, value);
    Ok(headers)
}

fn deadline(method: &str) -> Option<Duration> {
    matches!(
        method,
        wire::DESCRIBE | wire::FACTS_READ | wire::MODELS_LIST | wire::RESETS_SPEND
    )
    .then(|| wire::deadline(method))
}

/// The connector's HTTP client supplies the same TLS roots and single-request connection behavior as model endpoints.
#[derive(Debug, Clone)]
pub struct AddressClient {
    upstream: Upstream,
}

impl AddressClient {
    /// Uses the credential's keeping machine's HTTP transport.
    #[must_use]
    pub fn new(upstream: Upstream) -> Self {
        Self { upstream }
    }

    /// Calls one source method under its complete deadline; a sign-in is never an address credential.
    pub async fn call(&self, call: AddressCall<'_>) -> Result<Json, AddressError> {
        let mut secrets = call.secrets.to_vec();
        match &call.authorization {
            OpenAuthorization::None => {}
            OpenAuthorization::Bearer(value) | OpenAuthorization::Header { value, .. } => {
                secrets.push((*value).to_owned())
            }
        }
        if let Some(key) = call
            .params
            .pointer("/credential/key")
            .and_then(Json::as_str)
        {
            secrets.push(key.to_owned());
        }
        if let Some(fields) = call.params.get("secrets").and_then(Json::as_object) {
            secrets.extend(fields.values().filter_map(Json::as_str).map(str::to_owned));
        }
        let Some(deadline) = deadline(call.method) else {
            return Err(error(
                "unsupported",
                "an address does not implement that source method",
                false,
                &secrets,
            ));
        };
        let (auth, header) = match &call.authorization {
            OpenAuthorization::None => (AddressAuth::None, None),
            OpenAuthorization::Bearer(_) => (AddressAuth::Bearer, None),
            OpenAuthorization::Header { name, .. } => {
                (AddressAuth::Header, Some((*name).to_owned()))
            }
        };
        let binding = SourceAddress {
            url: call.url.to_owned(),
            auth,
            header,
            send_credential: false,
        };
        let url = binding.check(call.allow_http).map_err(|_| {
            error(
                "refused",
                "the source's address or scheme is not allowed",
                false,
                &secrets,
            )
        })?;
        if let Some(policy) = call.local_policy
            && policy.check_upstream(call.url).is_err()
        {
            return Err(error(
                "policy.upstream_not_allowed",
                "the machine's upstream policy does not allow this address",
                false,
                &secrets,
            ));
        }
        let headers = headers(&call.authorization, &secrets)?;
        let body = serde_json::to_vec(&address::AddressRequest::new(
            call.method,
            call.params.clone(),
        ))
        .map_err(|_| {
            error(
                "malformed",
                "the source request is not JSON",
                false,
                &secrets,
            )
        })?;
        if body.len() > REQUEST_BYTES {
            return Err(error(
                "too_large",
                "the source request is too large",
                false,
                &secrets,
            ));
        }
        let request = UpstreamRequest {
            method: Method::POST,
            url,
            headers,
            body: Bytes::from(body),
        };
        tokio::time::timeout(deadline, self.exchange(request, &secrets))
            .await
            .map_err(|_| {
                error(
                    "timeout",
                    "the source did not answer in time",
                    true,
                    &secrets,
                )
            })?
    }

    async fn exchange(
        &self,
        request: UpstreamRequest,
        secrets: &[String],
    ) -> Result<Json, AddressError> {
        let answer = self.upstream.send(request).await.map_err(|why| {
            let code = if matches!(why, UpstreamError::TimedOut(_)) {
                "timeout"
            } else {
                "unavailable"
            };
            error(code, "the source could not be reached", true, secrets)
        })?;
        let status = answer.status;
        let protected: Vec<&str> = secrets.iter().map(String::as_str).collect();
        if status != StatusCode::OK {
            return address::read_reply(status.as_u16(), &[])
                .map_err(|error| error.scrubbed(&protected));
        }
        if !answer
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
            })
        {
            return Err(error(
                "malformed",
                "the source did not answer with JSON",
                false,
                secrets,
            ));
        }
        let body = Limited::new(answer.body, REPLY_BYTES)
            .collect()
            .await
            .map_err(|why| {
                if why.is::<http_body_util::LengthLimitError>() {
                    error("too_large", "the source answered too much", false, secrets)
                } else {
                    error(
                        "unavailable",
                        "the source's reply could not be read",
                        true,
                        secrets,
                    )
                }
            })?
            .to_bytes();
        address::read_reply(status.as_u16(), &body).map_err(|error| error.scrubbed(&protected))
    }
}

/// The opened binding chosen by the provider's managers, under the two independent network policies.
pub struct AddressOptions {
    /// The canonical address metadata, including the explicit key opt-in.
    pub address: SourceAddress,
    /// Its own authorization token, opened beside the credential.
    pub authorization: Option<Secret>,
    /// The server's current plain-HTTP permission for this choice.
    pub allow_http: bool,
    /// The keeping machine's live policy; each call reads it again.
    pub policy: Arc<crate::policy::PolicyStore>,
    /// The same clock the connector uses for observations.
    pub clock: Arc<dyn Clock>,
    /// Other secret values the connector must remove from a source's words.
    pub known_secrets: Vec<String>,
}

struct Described {
    descriptor: SourceDescriptor,
    at: Time,
    invalid: bool,
}

/// An address implementing the same facts, models and reset interfaces as a locally registered program.
/// It cannot present credentials; the cached declaration is learned again after a day or a shape failure.
pub struct AddressSource {
    client: AddressClient,
    options: AddressOptions,
    described: Mutex<Described>,
    describing: tokio::sync::Mutex<()>,
}

impl AddressSource {
    /// Learns the address through its keeping machine before any fact or model is read.
    pub async fn connect(
        client: AddressClient,
        options: AddressOptions,
    ) -> Result<Self, SourceError> {
        let descriptor = Self::learn(&client, &options).await?;
        let at = options.clock.time();
        Ok(Self {
            client,
            options,
            described: Mutex::new(Described {
                descriptor,
                at,
                invalid: false,
            }),
            describing: tokio::sync::Mutex::new(()),
        })
    }

    fn protected(options: &AddressOptions, ask: Option<&SourceAsk>) -> Vec<String> {
        let mut secrets = options.known_secrets.clone();
        if let Some(token) = &options.authorization {
            secrets.push(token.expose().to_owned());
        }
        if let Some(ask) = ask {
            secrets.extend(
                ask.secrets
                    .values()
                    .map(|secret| secret.expose().to_owned()),
            );
            if let Some(key) = &ask.credential {
                secrets.push(key.key.expose().to_owned());
            }
        }
        secrets
    }

    async fn request(
        client: &AddressClient,
        options: &AddressOptions,
        method: &str,
        params: &Json,
        secrets: &[String],
    ) -> Result<Json, SourceError> {
        let token = options.authorization.as_ref().map(Secret::expose);
        let authorization = match options.address.auth {
            AddressAuth::None => OpenAuthorization::None,
            AddressAuth::Bearer => OpenAuthorization::Bearer(token.ok_or_else(|| {
                SourceError::new(
                    SourceErrorCode::Unauthorized,
                    "the source address has no authorization token",
                )
            })?),
            AddressAuth::Header => OpenAuthorization::Header {
                name: options.address.header.as_deref().ok_or_else(|| {
                    SourceError::new(
                        SourceErrorCode::Malformed,
                        "the source address has no authorization header name",
                    )
                })?,
                value: token.ok_or_else(|| {
                    SourceError::new(
                        SourceErrorCode::Unauthorized,
                        "the source address has no authorization token",
                    )
                })?,
            },
        };
        let local_policy = options.policy.policy();
        client
            .call(AddressCall {
                url: &options.address.url,
                authorization,
                method,
                params,
                allow_http: options.allow_http,
                local_policy: local_policy.as_ref(),
                secrets,
            })
            .await
    }

    async fn learn(
        client: &AddressClient,
        options: &AddressOptions,
    ) -> Result<SourceDescriptor, SourceError> {
        let secrets = Self::protected(options, None);
        let raw = Self::request(
            client,
            options,
            wire::DESCRIBE,
            &serde_json::json!({}),
            &secrets,
        )
        .await?;
        let raw = super::reports::scrub_value(&raw, &secrets);
        let mut descriptor: SourceDescriptor = serde_json::from_value(raw).map_err(|_| {
            SourceError::new(
                SourceErrorCode::Malformed,
                "the source's descriptor has another shape",
            )
        })?;
        // A transport's effective interfaces cannot include a presenter that is forbidden on that transport.
        descriptor.presents = None;
        descriptor.check().map_err(|why| {
            SourceError::new(SourceErrorCode::Malformed, why.to_string())
                .scrubbed(&secrets.iter().map(String::as_str).collect::<Vec<_>>())
        })?;
        Ok(descriptor)
    }

    async fn current(&self) -> Result<SourceDescriptor, SourceError> {
        let _one = self.describing.lock().await;
        let now = self.options.clock.time();
        {
            let cached = self
                .described
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let age = now
                .get()
                .signed_duration_since(cached.at.get())
                .to_std()
                .ok();
            if !cached.invalid && age.is_some_and(|age| age < limits::DESCRIBE_EVERY) {
                return Ok(cached.descriptor.clone());
            }
        }
        let descriptor = Self::learn(&self.client, &self.options).await?;
        *self
            .described
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Described {
            descriptor: descriptor.clone(),
            at: now,
            invalid: false,
        };
        Ok(descriptor)
    }

    fn descriptor(&self) -> SourceDescriptor {
        self.described
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .descriptor
            .clone()
    }

    fn prepare(&self, descriptor: &SourceDescriptor, mut ask: SourceAsk) -> SourceAsk {
        if !self.options.address.send_credential || !descriptor.wants_key() {
            ask.credential = None;
        }
        ask.secrets
            .retain(|name, _| descriptor.secret_fields().any(|field| field.name == *name));
        ask
    }

    fn failed(&self, error: SourceError) -> SourceError {
        if matches!(
            error.code,
            SourceErrorCode::Malformed | SourceErrorCode::Unsupported
        ) {
            self.described
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .invalid = true;
        }
        error
    }

    async fn method(
        &self,
        method: &str,
        ask: SourceAsk,
        reset: Option<orch_provider_contract::source::ResetRequest>,
    ) -> Result<(Json, bool), SourceError> {
        let descriptor = self.current().await?;
        if !descriptor.implements(method) {
            return Err(SourceError::unsupported(method));
        }
        let secrets = Self::protected(&self.options, Some(&ask));
        let ask = self.prepare(&descriptor, ask);
        let used = ask.credential.is_some();
        let params = if let Some(reset) = reset {
            serde_json::to_value(ResetSpendParams {
                ask,
                reset_id: reset.reset_id,
                idempotency_key: reset.idempotency_key,
            })
        } else {
            serde_json::to_value(ask)
        }
        .map_err(|_| {
            SourceError::new(
                SourceErrorCode::Malformed,
                "the source parameters could not be written",
            )
        })?;
        let reply = Self::request(&self.client, &self.options, method, &params, &secrets)
            .await
            .map_err(|error| self.failed(error))?;
        Ok((super::reports::scrub_value(&reply, &secrets), used))
    }
}

#[async_trait]
impl FactsSource for AddressSource {
    fn describe(&self) -> SourceDescriptor {
        self.descriptor()
    }

    async fn read_facts(&self, ask: SourceAsk) -> Result<FactsReport, SourceError> {
        let (mut report, used) = self.method(wire::FACTS_READ, ask, None).await?;
        if !report.is_object() {
            return Err(self.failed(SourceError::new(SourceErrorCode::Malformed, "facts")));
        }
        if !used && let Some(members) = report.as_object_mut() {
            members.remove("credential_verified");
            members.remove("credential");
        }
        if super::reports::read(&report, self.options.clock.time(), used, &[])
            .map_or(true, |read| !read.notes.is_empty())
        {
            self.described
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .invalid = true;
        }
        Ok(FactsReport(report))
    }

    async fn spend_reset(
        &self,
        ask: SourceAsk,
        reset: orch_provider_contract::source::ResetRequest,
    ) -> Result<ResetReport, SourceError> {
        let (report, used) = self.method(wire::RESETS_SPEND, ask, Some(reset)).await?;
        let mut report: ResetReport = serde_json::from_value(report).map_err(|_| {
            self.failed(SourceError::new(SourceErrorCode::Malformed, "reset result"))
        })?;
        if !used
            && let Some(facts) = report
                .facts
                .as_mut()
                .and_then(|facts| facts.0.as_object_mut())
        {
            facts.remove("credential_verified");
            facts.remove("credential");
        }
        Ok(report)
    }
}

#[async_trait]
impl ModelsSource for AddressSource {
    fn describe(&self) -> SourceDescriptor {
        self.descriptor()
    }

    async fn list_models(&self, ask: SourceAsk) -> Result<ModelsReport, SourceError> {
        let (report, _) = self.method(wire::MODELS_LIST, ask, None).await?;
        let report: ModelsReport = serde_json::from_value(report)
            .map_err(|_| self.failed(SourceError::new(SourceErrorCode::Malformed, "models")))?;
        let raw = serde_json::to_value(&report.models)
            .map_err(|_| self.failed(SourceError::new(SourceErrorCode::Malformed, "models")))?;
        if super::reports::models(&raw, &[]).map_or(true, |read| !read.notes.is_empty()) {
            self.described
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .invalid = true;
        }
        Ok(report)
    }
}

//! Anthropic Messages wire-protocol implementation.
//!
//! [`AnthropicMessages`] is the [`WireProtocol`] of the Messages format of Anthropic, `anthropic-messages`: what
//! the provider role of a connector uses to forward a harness's model calls to an endpoint that speaks it. It is
//! a plain relay, like the Responses protocol beside it. A request passes through as it came, with nothing
//! rewritten but the credential; the answer passes back as it came, and is only read on its way to count what
//! it used. Which harness speaks this format is a matter of agent adapters, not of this package.
//!
//! | Module | What it knows |
//! |--------|---------------|
//! | [`requests`] | The requests that pass (`POST /v1/messages`, `POST /v1/messages/count_tokens`, `GET /v1/models`) and where the model is (the member `model` of the body) |
//! | [`authorization`] | How the key and the endpoint's headers and query are presented, and the shape of a refusal |
//! | [`usage`] | Reading input, output, cached and reasoning tokens and a stated cost from a stream or a plain answer |
//! | [`models`] | Reading an endpoint's list of models into suggestions, and writing a lease's list |
//! | [`credentials`] | The one form issuable from a key: the key as it is |
//! | [`decimal`] | Exact decimal text for prices and costs |
//!
//! The formats are the vendor's as documented in October 2026 (its API reference and guides): the streaming
//! events and the `usage` object of a message, the list of models, the error document and the headers that
//! carry a key.

#![forbid(unsafe_code)]

pub mod authorization;
pub mod credentials;
pub mod decimal;
pub mod models;
pub mod requests;
pub mod usage;

use bytes::Bytes;
use futures::future::BoxFuture;
use http::{HeaderMap, Method, Response, StatusCode};
use orch_agent_contract::credentials::{CredentialForm, IssuedCredential};
use orch_agent_contract::discovery::ModelSuggestion;
use orch_model_contract::credentials::{Credential, WireError};
use orch_model_contract::endpoint::{AuthMode, AuthorizeError, EndpointOptions, standard_options};
use orch_model_contract::protocol::{ANTHROPIC_MESSAGES, SecretHeaders, WireProtocol};
use orch_model_contract::usage::UsageObserver;
use orch_types::Field;
use url::Url;

/// The wire protocol of the Messages format of Anthropic.
#[derive(Debug, Clone, Copy, Default)]
pub struct AnthropicMessages;

impl AnthropicMessages {
    /// The answer to a harness's `GET /v1/models` under a lease: exactly the named models, in the shape of the
    /// vendor's list. The model contract has no method for this, and the forwarder never forwards that request.
    #[must_use]
    pub fn models_response(&self, names: &[String]) -> Response<Bytes> {
        models::list_response(names)
    }
}

impl WireProtocol for AnthropicMessages {
    fn name(&self) -> &'static str {
        ANTHROPIC_MESSAGES
    }

    fn allowed(&self, method: &Method, path: &str) -> bool {
        requests::allowed(method, path)
    }

    fn model_of(&self, path: &str, body: &[u8]) -> Option<String> {
        requests::model_of(path, body)
    }

    fn endpoint_options(&self) -> Vec<Field> {
        standard_options()
    }

    fn default_auth(&self) -> AuthMode {
        authorization::DEFAULT_AUTH
    }

    fn authorize(
        &self,
        headers: &mut HeaderMap,
        credential: Option<&Credential>,
        options: &EndpointOptions,
        secrets: &SecretHeaders,
    ) -> Result<Vec<(String, String)>, AuthorizeError> {
        authorization::present(headers, credential, options, secrets)
    }

    fn deny(&self, status: StatusCode, code: &str, message: &str) -> Response<Bytes> {
        authorization::deny(status, code, message)
    }

    fn usage_observer(&self) -> Box<dyn UsageObserver> {
        Box::new(usage::MessagesUsage::new())
    }

    fn models_request(&self) -> Option<(Method, &'static str)> {
        Some(models::request())
    }

    fn parse_models(&self, body: &[u8]) -> Vec<ModelSuggestion> {
        models::parse(body)
    }

    fn next_models_request(&self, current: &str, body: &[u8]) -> Result<Option<String>, WireError> {
        orch_model_contract::protocol::next_models_page(current, body, "after_id")
    }

    fn completion_request(
        &self,
        model: &str,
        system: &str,
        input: &str,
        max_output_tokens: i64,
    ) -> Option<(Method, &'static str, Bytes)> {
        Some((Method::POST, "/v1/messages", Bytes::from(serde_json::json!({
            "model": model, "system": system, "messages": [{"role": "user", "content": input}],
            "max_tokens": max_output_tokens, "stream": false,
        }).to_string())))
    }

    fn completion_text(&self, body: &[u8]) -> Result<String, WireError> {
        let bad =
            || WireError::Internal("the response contains no completed text answer".to_owned());
        let reply: serde_json::Value = serde_json::from_slice(body).map_err(|_| bad())?;
        if reply["type"] == "error"
            || reply
                .get("stop_reason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| !matches!(reason, "end_turn" | "stop_sequence"))
        {
            return Err(bad());
        }
        let text: String = reply
            .get("content")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter(|part| part["type"] == "text")
            .filter_map(|part| part["text"].as_str())
            .collect();
        if text.is_empty() {
            Err(bad())
        } else {
            Ok(text)
        }
    }

    fn credential_forms(&self) -> Vec<CredentialForm> {
        credentials::forms()
    }

    fn issue_credential<'a>(
        &'a self,
        form: &'a str,
        key: &'a Credential,
        upstream: &'a Url,
    ) -> BoxFuture<'a, Result<IssuedCredential, WireError>> {
        let _ = upstream;
        Box::pin(std::future::ready(credentials::issue(form, key)))
    }
}

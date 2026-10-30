//! OpenAI Responses wire-protocol implementation.
//!
//! [`OpenAiResponses`] is the [`WireProtocol`] of the Responses format of OpenAI, `openai-responses`: what the
//! provider role of a connector uses to forward a harness's model calls to an endpoint that speaks it. It is a
//! plain relay. A request passes through as it came, with nothing rewritten but the credential; the answer
//! passes back as it came, and is only read on its way to count what it used.
//!
//! | Module | What it knows |
//! |--------|---------------|
//! | [`requests`] | The requests that pass (`POST /responses`, `POST /responses/compact`, `GET /models`) and where the model is (the member `model` of the body) |
//! | [`chat`] | Chat Completions pass-through using the same authorization, model discovery and bounded usage reader |
//! | [`embeddings`] | The Embeddings format: its pass-through, and the batch of texts Orch turns into vectors to search by meaning |
//! | [`authorization`] | How the key and the endpoint's headers and query are presented, and the shape of a refusal |
//! | [`usage`] | Reading input, output, cached and reasoning tokens and a stated cost from a stream or a plain answer |
//! | [`models`] | Reading an endpoint's list of models into suggestions, and writing a lease's list |
//! | [`credentials`] | The one form issuable from a key: the key as it is |
//! | [`decimal`] | Exact decimal text for prices and costs |
//!
//! The formats are the vendor's as documented in October 2026 (its published API description and guides): the
//! streaming events and the `usage` object of a response, the list of models, and the error document.

#![forbid(unsafe_code)]

pub mod authorization;
pub mod chat;
pub mod credentials;
pub mod decimal;
pub mod embeddings;
pub mod models;
pub mod requests;
pub mod usage;

pub use chat::OpenAiChat;
pub use embeddings::OpenAiEmbeddings;

use bytes::Bytes;
use futures::future::BoxFuture;
use http::{HeaderMap, Method, Response, StatusCode};
use orch_agent_contract::credentials::{CredentialForm, IssuedCredential};
use orch_agent_contract::discovery::ModelSuggestion;
use orch_model_contract::credentials::{Credential, WireError};
use orch_model_contract::endpoint::{AuthMode, AuthorizeError, EndpointOptions, standard_options};
use orch_model_contract::protocol::{OPENAI_RESPONSES, SecretHeaders, WireProtocol};
use orch_model_contract::usage::UsageObserver;
use orch_types::Field;
use url::Url;

/// The wire protocol of the Responses format of OpenAI.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenAiResponses;

impl OpenAiResponses {
    /// The answer to a harness's `GET /models` under a lease: exactly the named models, in the shape of the
    /// vendor's list. The model contract has no method for this, and the forwarder never forwards that request.
    #[must_use]
    pub fn models_response(&self, names: &[String]) -> Response<Bytes> {
        models::list_response(names)
    }
}

impl WireProtocol for OpenAiResponses {
    fn name(&self) -> &'static str {
        OPENAI_RESPONSES
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
        Box::new(usage::ResponsesUsage::new())
    }

    fn models_request(&self) -> Option<(Method, &'static str)> {
        Some(models::request())
    }

    fn parse_models(&self, body: &[u8]) -> Vec<ModelSuggestion> {
        models::parse(body)
    }

    fn next_models_request(&self, current: &str, body: &[u8]) -> Result<Option<String>, WireError> {
        orch_model_contract::protocol::next_models_page(current, body, "after")
    }

    fn completion_request(
        &self,
        model: &str,
        system: &str,
        input: &str,
        max_output_tokens: i64,
    ) -> Option<(Method, &'static str, Bytes)> {
        Some((
            Method::POST,
            "/responses",
            Bytes::from(
                serde_json::json!({
                    "model": model, "instructions": system, "input": input,
                    "max_output_tokens": max_output_tokens, "stream": false,
                })
                .to_string(),
            ),
        ))
    }

    fn completion_text(&self, body: &[u8]) -> Result<String, WireError> {
        let bad =
            || WireError::Internal("the response contains no completed text answer".to_owned());
        let reply: serde_json::Value = serde_json::from_slice(body).map_err(|_| bad())?;
        if reply.get("error").is_some_and(|error| !error.is_null())
            || reply
                .get("status")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|status| status != "completed")
        {
            return Err(bad());
        }
        let text: String = reply
            .get("output")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| item["type"] == "message")
            .flat_map(|item| {
                item.get("content")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter(|part| part["type"] == "output_text")
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

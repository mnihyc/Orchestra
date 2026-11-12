//! WireProtocol request allowlists, authorization as an endpoint's options say, and model discovery; a plain relay per protocol.
//!
//! A wire protocol knows the shape of one model API: which requests are allowed below the upstream base, where
//! the model name is, how to add the credential and what an endpoint may configure about that, how to shape
//! an error, how to read usage from a response, and which credential forms can be issued from a key. The
//! forwarder is a plain relay: a request passes through in the protocol it arrived in, to the endpoint of the
//! upstream that speaks it, with nothing rewritten but the credential. Serving a protocol the upstream does
//! not speak is the job of a translator ([`crate::translator`]).
//!
//! | Name | Requests allowed below the upstream base | Model | Credential header, unless the endpoint says otherwise |
//! |------|-------------------------------------------|-------|-------------------------------------------------------|
//! | `openai-responses` | `POST /responses`, `GET /models`, endpoint-declared `POST /alpha/search` | body member `model` | `Authorization: Bearer <key>` |
//! | `openai-chat` | `POST /chat/completions`, `GET /models` | body member `model` | `Authorization: Bearer <key>` |
//! | `anthropic-messages` | `POST /v1/messages`, `POST /v1/messages/count_tokens`, `GET /v1/models` | body member `model` | `Authorization: Bearer <key>`; `x-api-key: <key>` where the endpoint's `auth` says so |
//! | `openai-embeddings` | `POST /embeddings`, `GET /models` | body member `model` | `Authorization: Bearer <key>` |
//!
//! All four formats have built-in pass-through relays. An agent still declares which it speaks; no agent speaks
//! the embeddings format, which is for the requests Orch makes itself to search by meaning
//! ([`WireProtocol::embedding_request`]).
//! The tables are data here ([`OPENAI_RESPONSES_REQUESTS`] and the others), so that the protocols and the
//! forwarder read one definition.

use std::collections::BTreeMap;

use bytes::Bytes;
use futures::future::BoxFuture;
use http::{HeaderMap, Method, Response, StatusCode};
use orch_agent_contract::credentials::CredentialForm;
use orch_agent_contract::discovery::ModelSuggestion;
use orch_plugin_contract::Secret;
use orch_types::Field;
use url::Url;

use crate::credentials::{Credential, WireError, issue_standard, standard_forms};
use crate::endpoint::{
    AuthMode, AuthorizeError, EndpointOptions, authorize_joined, standard_options,
};
use crate::usage::UsageObserver;
use orch_agent_contract::credentials::IssuedCredential;

/// The name of the Responses format of OpenAI.
pub const OPENAI_RESPONSES: &str = "openai-responses";
/// The name of the Chat Completions format of OpenAI.
pub const OPENAI_CHAT: &str = "openai-chat";
/// The name of the Messages format of Anthropic.
pub const ANTHROPIC_MESSAGES: &str = "anthropic-messages";
/// The name of the Embeddings format of OpenAI, which many servers of embedding models speak.
pub const OPENAI_EMBEDDINGS: &str = "openai-embeddings";
/// The protocol that makes vectors, by what it does and not by whose format it is.
pub const EMBEDDINGS: &str = OPENAI_EMBEDDINGS;

/// Every wire protocol a provider may be declared with, in the order the constraint catalogue lists them.
pub const SUPPORTED: [&str; 4] = [
    OPENAI_CHAT,
    OPENAI_RESPONSES,
    ANTHROPIC_MESSAGES,
    OPENAI_EMBEDDINGS,
];

/// The parameter field declarations of a wire format. Generic option resolution
/// consumes these alongside the request allowlists rather than selecting a vendor.
#[derive(Clone, Copy, Default)]
pub(crate) struct ParameterFormat {
    /// Cache directive vocabulary and exact native positions, independent of user content.
    pub cache: Option<CacheFormat>,
    /// Tier field and its explicit ordinary value, when the format reports tiers.
    pub tier: Option<(&'static str, &'static str)>,
    /// Some interfaces encode ordinary inference speed only by omitting the field.
    pub omit_ordinary_tier: bool,
    /// Independent capacity field, ordinary request selection and ordinary reported class.
    pub capacity_tier: Option<(&'static str, &'static str, &'static str)>,
    /// Neutral native-search option, whose tool mapping is declared by this format.
    pub web_search: Option<&'static str>,
    /// Effort field and optional nested member.
    pub effort: Option<(&'static str, Option<&'static str>)>,
    /// Reasoning object, mode member and ordinary mode; additional modes require model enablement.
    pub reasoning_mode: Option<(&'static str, &'static str, &'static str)>,
    /// Program object, member and explicit ordinary value.
    pub program: Option<(&'static str, &'static str, &'static str)>,
    /// An ordinary program selection is represented by absence on this interface.
    pub omit_ordinary_program: bool,
    /// Reported tier and its pricing alias; the reverse maps a requested cap alias.
    pub tier_aliases: &'static [(&'static str, &'static str)],
}

#[derive(Clone, Copy)]
pub(crate) struct CacheFormat {
    pub field: &'static str,
    pub lifetime: &'static str,
    pub ordinary: &'static str,
    pub lifetimes: &'static [&'static str],
    pub pointers: &'static [&'static str],
}

/// Canonical format data, independent of the vendor hosting an endpoint.
pub(crate) fn parameter_format(protocol: &str) -> ParameterFormat {
    match protocol {
        OPENAI_RESPONSES => ParameterFormat {
            web_search: Some("web_search"),
            tier: Some(("service_tier", "default")),
            effort: Some(("reasoning", Some("effort"))),
            reasoning_mode: Some(("reasoning", "mode", "standard")),
            program: Some(("access_programs", "cyber", "standard")),
            omit_ordinary_program: true,
            tier_aliases: &[("priority", "fast")],
            ..Default::default()
        },
        OPENAI_CHAT => ParameterFormat {
            tier: Some(("service_tier", "default")),
            effort: Some(("reasoning_effort", None)),
            tier_aliases: &[("priority", "fast")],
            ..Default::default()
        },
        ANTHROPIC_MESSAGES => ParameterFormat {
            cache: Some(CacheFormat {
                field: "cache_control",
                lifetime: "ttl",
                ordinary: "5m",
                lifetimes: &["5m", "1h"],
                pointers: &[
                    "/cache_control",
                    "/system/*/cache_control",
                    "/tools/*/cache_control",
                    "/messages/*/content/*/cache_control",
                    "/messages/*/content/*/content/*/cache_control",
                ],
            }),
            web_search: Some("web_search"),
            tier: Some(("speed", "standard")),
            omit_ordinary_tier: true,
            capacity_tier: Some(("service_tier", "standard_only", "standard")),
            effort: Some(("output_config", Some("effort"))),
            ..Default::default()
        },
        _ => ParameterFormat::default(),
    }
}

/// The requests that `openai-responses` lets through, below the upstream base.
pub const OPENAI_RESPONSES_REQUESTS: [(&str, &str); 3] = [
    ("POST", "/responses"),
    ("GET", "/models"),
    ("POST", "/alpha/search"),
];
/// The requests that `openai-chat` lets through, below the upstream base.
pub const OPENAI_CHAT_REQUESTS: [(&str, &str); 2] =
    [("POST", "/chat/completions"), ("GET", "/models")];
/// The requests that `openai-embeddings` lets through, below the upstream base.
pub const OPENAI_EMBEDDINGS_REQUESTS: [(&str, &str); 2] =
    [("POST", "/embeddings"), ("GET", "/models")];
/// The requests that `anthropic-messages` lets through, below the upstream base.
pub const ANTHROPIC_MESSAGES_REQUESTS: [(&str, &str); 3] = [
    ("POST", "/v1/messages"),
    ("POST", "/v1/messages/count_tokens"),
    ("GET", "/v1/models"),
];

/// Whether a request of `method` to `path` below the upstream base is one of `allowed`. The query is not
/// part of the path; a path matches exactly.
#[must_use]
pub fn is_allowed(allowed: &[(&str, &str)], method: &Method, path: &str) -> bool {
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    allowed.iter().any(|(allowed_method, allowed_path)| {
        *allowed_method == method.as_str() && *allowed_path == path
    })
}

/// The model a request names in the member `model` of its JSON body: the routing name, as it is written.
#[must_use]
pub fn model_in_body(body: &[u8]) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_slice(body).ok()?;
    parsed.get("model")?.as_str().map(str::to_owned)
}

/// The secret header values of an endpoint, by header name.
pub type SecretHeaders = BTreeMap<String, Secret>;

/// A hundred pages allow large catalogues while bounding a peer that keeps promising another page.
pub const MODEL_DISCOVERY_MAX_PAGES: usize = 100;
/// Ten thousand distinct routing names bound discovery memory and the resulting suggestions message: the
/// provider contract's bound of a list of models, written once there.
pub const MODEL_DISCOVERY_MAX_MODELS: usize = orch_provider_contract::limits::MODELS_MAX;

/// The next page of a model list with a vendor cursor, or an explicit next link which the runtime must
/// validate against the original endpoint before presenting any credential.
pub fn next_models_page(
    current: &str,
    body: &[u8],
    cursor_key: &str,
) -> Result<Option<String>, WireError> {
    let malformed = || WireError::Internal("the model list has invalid pagination".to_owned());
    let page: serde_json::Value = serde_json::from_slice(body).map_err(|_| malformed())?;
    for key in ["next", "next_page"] {
        if let Some(next) = page
            .get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|next| !next.is_empty())
        {
            return Ok(Some(next.to_owned()));
        }
    }
    if page.get("has_more").and_then(serde_json::Value::as_bool) != Some(true) {
        return Ok(None);
    }
    let cursor = page
        .get("last_id")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            page.get("data")
                .and_then(serde_json::Value::as_array)
                .and_then(|data| data.last())
                .and_then(|model| model.get("id"))
                .and_then(serde_json::Value::as_str)
        })
        .filter(|cursor| !cursor.is_empty())
        .ok_or_else(malformed)?;
    let (path, old) = current.split_once('?').unwrap_or((current, ""));
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in url::form_urlencoded::parse(old.as_bytes()) {
        if key != cursor_key {
            query.append_pair(&key, &value);
        }
    }
    query.append_pair(cursor_key, cursor);
    Ok(Some(format!("{path}?{}", query.finish())))
}

/// An interface's required header, selected from the FINAL outgoing JSON body.
#[derive(Debug, Clone, Copy)]
pub struct RequestHeaderRule {
    /// JSON pointer in the actual emitted body.
    pub pointer: &'static str,
    /// A complete JSON literal, preserving string/number/null distinctions.
    pub equals: &'static str,
    /// Header owned by this interface requirement.
    pub name: &'static str,
    /// Required wire value.
    pub value: &'static str,
    /// Whether to retain the program/endpoint's existing comma-list tokens.
    pub join: bool,
}

/// A recognized routing list, containing model names or one-field model-name objects.
#[derive(Clone, Copy, Debug)]
pub struct ModelListField {
    /// JSON pointer of the optional list in the actual wire body.
    pub pointer: &'static str,
    /// None means strings; Some names the sole member of each routing object.
    pub member: Option<&'static str>,
}

/// Read every declared routing name without changing the request or choosing a fallback.
pub fn model_candidates(
    primary: Option<String>,
    body: &[u8],
    lists: &[ModelListField],
) -> Result<Vec<String>, &'static str> {
    let mut names = Vec::new();
    if let Some(primary) = primary {
        if primary.trim().is_empty() {
            return Err("the selected model name is empty");
        }
        names.push(primary);
    }
    if lists.is_empty() {
        return Ok(names);
    }
    let body: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| "the model routing body cannot be read")?;
    for list in lists {
        let Some(value) = body.pointer(list.pointer) else {
            continue;
        };
        let values = value
            .as_array()
            .ok_or("model routing candidates must be a list")?;
        for value in values {
            let value = if let Some(member) = list.member {
                let object = value
                    .as_object()
                    .filter(|object| object.len() == 1)
                    .ok_or("a routing candidate must contain only its model name")?;
                object
                    .get(member)
                    .ok_or("a routing candidate names no model")?
            } else {
                value
            };
            let name = value
                .as_str()
                .filter(|name| !name.trim().is_empty())
                .ok_or("every routing candidate must name a model")?;
            if !names.iter().any(|known| known == name) {
                names.push(name.to_owned());
            }
        }
    }
    Ok(names)
}

/// One implementation per wire protocol, used by the provider role.
pub trait WireProtocol: Send + Sync + 'static {
    /// Its name, such as `openai-responses`.
    fn name(&self) -> &'static str;

    /// Whether a request is one the protocol lets through; `path` is below the upstream base.
    fn allowed(&self, method: &Method, path: &str) -> bool;

    /// The model a request names, from its path or its body.
    fn model_of(&self, path: &str, body: &[u8]) -> Option<String>;

    /// Extra routing carriers recognized by this interface, without claiming endpoint support.
    fn model_lists(&self) -> &'static [ModelListField] {
        &[]
    }

    /// The primary and every possible alternative, in their original order.
    fn model_candidates(&self, path: &str, body: &[u8]) -> Result<Vec<String>, &'static str> {
        model_candidates(self.model_of(path, body), body, self.model_lists())
    }

    /// What an endpoint of this protocol may configure beyond its base URL, in the shape every form uses.
    /// The built-in protocols declare [`standard_options`].
    fn endpoint_options(&self) -> Vec<Field> {
        standard_options()
    }

    /// How the key is presented when an endpoint's options say nothing about it.
    fn default_auth(&self) -> AuthMode {
        AuthMode::Bearer
    }

    /// Presents the credential the way the endpoint's options say, then the endpoint's headers and secret
    /// headers; `None` for a credential is for an endpoint that takes none. Returns the query parameters
    /// the endpoint adds to every request. See [`authorize`](crate::endpoint::authorize).
    fn authorize(
        &self,
        headers: &mut HeaderMap,
        credential: Option<&Credential>,
        options: &EndpointOptions,
        secrets: &SecretHeaders,
    ) -> Result<Vec<(String, String)>, AuthorizeError> {
        authorize_joined(
            headers,
            credential,
            options,
            secrets,
            self.default_auth(),
            self.joined_headers(),
        )
    }

    /// Interface-declared header lists. Scalar/authentication headers are not guessed as lists.
    fn joined_headers(&self) -> &'static [&'static str] {
        &[]
    }

    /// Required headers whose predicates describe the actual final request, not a requested default.
    fn request_header_rules(&self) -> &'static [RequestHeaderRule] {
        &[]
    }

    /// Apply declared requirements after final body validation and credential presentation.
    fn finalize_request_headers(
        &self,
        headers: &mut HeaderMap,
        body: &[u8],
    ) -> Result<(), AuthorizeError> {
        let rules = self.request_header_rules();
        if rules.is_empty() {
            return Ok(());
        }
        let Ok(body) = serde_json::from_slice::<serde_json::Value>(body) else {
            return Ok(());
        };
        for rule in rules {
            let expected: serde_json::Value = serde_json::from_str(rule.equals)
                .map_err(|_| AuthorizeError::BadHeader(rule.name.into()))?;
            if body.pointer(rule.pointer) != Some(&expected) {
                continue;
            }
            let name = http::HeaderName::from_bytes(rule.name.as_bytes())
                .map_err(|_| AuthorizeError::BadHeader(rule.name.into()))?;
            let value = http::HeaderValue::from_str(rule.value)
                .map_err(|_| AuthorizeError::BadHeader(rule.name.into()))?;
            if rule.join {
                crate::endpoint::append_comma_header(headers, name, value)?;
            } else {
                headers.insert(name, value);
            }
        }
        Ok(())
    }

    /// An error in the protocol's own shape, for a request the forwarder refuses.
    fn deny(&self, status: StatusCode, code: &str, message: &str) -> Response<Bytes>;

    /// A reader of one response's usage as it streams by, best-effort.
    fn usage_observer(&self) -> Box<dyn UsageObserver>;

    /// Side requests that require an endpoint binding and this turn's captured native capability.
    /// Merely appearing in a protocol's method/path table never grants that capability.
    fn native_capability(&self, _path: &str) -> Option<&'static str> {
        None
    }

    /// The same byte reader may use the admitted request's operation to interpret a side reply.
    fn usage_observer_at(&self, _path: &str, _body: &[u8]) -> Box<dyn UsageObserver> {
        self.usage_observer()
    }

    /// The request that lists the models of an endpoint, for discovery: its method and its path below the
    /// base. `None` when the protocol has none.
    fn models_request(&self) -> Option<(Method, &'static str)>;

    /// Reads the answer to [`WireProtocol::models_request`] into suggestions: the routing name, the proposed
    /// display name, and a rate where the list states prices.
    fn parse_models(&self, body: &[u8]) -> Vec<ModelSuggestion>;

    /// The next model-list request path, after this page; a runtime enforces page, model and endpoint bounds.
    fn next_models_request(&self, current: &str, body: &[u8]) -> Result<Option<String>, WireError> {
        let _ = (current, body);
        Ok(None)
    }

    /// Builds one non-streaming text request made on the system's behalf; absent when unsupported.
    fn completion_request(
        &self,
        model: &str,
        system: &str,
        input: &str,
        max_output_tokens: i64,
    ) -> Option<(Method, &'static str, Bytes)> {
        let _ = (model, system, input, max_output_tokens);
        None
    }

    /// Builds the same completion using its actual endpoint's declared wire options.
    fn completion_request_at(
        &self,
        options: &EndpointOptions,
        model: &str,
        system: &str,
        input: &str,
        max_output_tokens: i64,
    ) -> Option<(Method, &'static str, Bytes)> {
        let _ = options;
        self.completion_request(model, system, input, max_output_tokens)
    }

    /// Reads the text of a successful non-streaming completion, without reasoning or tool calls.
    fn completion_text(&self, body: &[u8]) -> Result<String, WireError> {
        let _ = body;
        Err(WireError::Internal(
            "this protocol cannot read a text completion".to_owned(),
        ))
    }

    /// Tool-capable dialogue generation. Protocols without generation, including
    /// embeddings, leave this unsupported instead of pretending to be a chat model.
    fn generation_request(
        &self,
        model: &str,
        input: &crate::generation::GenerationInput,
        stream: bool,
    ) -> Option<(Method, &'static str, Bytes)> {
        let _ = (model, input, stream);
        None
    }

    /// Builds the same dialogue using the actual endpoint's declared wire options.
    fn generation_request_at(
        &self,
        options: &EndpointOptions,
        model: &str,
        input: &crate::generation::GenerationInput,
        stream: bool,
    ) -> Option<(Method, &'static str, Bytes)> {
        let _ = options;
        self.generation_request(model, input, stream)
    }

    /// Reads a non-streaming dialogue answer, retaining protocol continuation blocks.
    fn generation_output(
        &self,
        body: &[u8],
    ) -> Result<crate::generation::GenerationOutput, WireError> {
        let _ = body;
        Err(WireError::Internal(
            "this protocol cannot generate dialogue".to_owned(),
        ))
    }

    /// A fresh stream reader for this generation protocol.
    fn generation_stream(&self) -> Option<Box<dyn crate::generation::GenerationStream>> {
        None
    }

    /// Builds one request that turns texts into vectors, made on the system's behalf; absent when unsupported.
    /// `dimensions` asks a model that can shorten its vectors for that length; a model that cannot ignores it.
    fn embedding_request(
        &self,
        model: &str,
        inputs: &[String],
        dimensions: Option<u32>,
    ) -> Option<(Method, &'static str, Bytes)> {
        let _ = (model, inputs, dimensions);
        None
    }

    /// Reads the vectors of a successful embeddings answer: one for each of the `inputs` texts, in their order.
    fn embedding_vectors(&self, body: &[u8], inputs: usize) -> Result<Vec<Vec<f32>>, WireError> {
        let _ = (body, inputs);
        Err(WireError::Internal(
            "this protocol cannot read embeddings".to_owned(),
        ))
    }

    /// The forms the provider role can issue from an API key. Always includes `api_key`: the key as it is.
    /// A protocol whose upstream can mint narrower or expiring keys declares those as well.
    fn credential_forms(&self) -> Vec<CredentialForm> {
        standard_forms()
    }

    /// Issues `key` as the credential `form`, for an upstream at `upstream`. The form `api_key` is the key as
    /// it is; any other form is for the protocol that declared it.
    fn issue_credential<'a>(
        &'a self,
        form: &'a str,
        key: &'a Credential,
        upstream: &'a Url,
    ) -> BoxFuture<'a, Result<IssuedCredential, WireError>> {
        let _ = upstream;
        Box::pin(async move { issue_standard(form, key) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::ObservedUsage;
    use futures::executor::block_on;
    use http::HeaderValue;

    /// The least a protocol does: the three tables it needs and an error in a shape of its own.
    struct Plain;

    struct Quiet;
    impl UsageObserver for Quiet {
        fn observe(&mut self, _: &[u8]) {}
        fn usage(&self) -> Option<ObservedUsage> {
            None
        }
    }

    impl WireProtocol for Plain {
        fn name(&self) -> &'static str {
            OPENAI_RESPONSES
        }
        fn allowed(&self, method: &Method, path: &str) -> bool {
            is_allowed(&OPENAI_RESPONSES_REQUESTS, method, path)
        }
        fn model_of(&self, _: &str, body: &[u8]) -> Option<String> {
            model_in_body(body)
        }
        fn deny(&self, status: StatusCode, code: &str, message: &str) -> Response<Bytes> {
            let body = format!("{{\"error\":{{\"code\":\"{code}\",\"message\":\"{message}\"}}}}");
            Response::builder()
                .status(status)
                .body(Bytes::from(body))
                .unwrap()
        }
        fn usage_observer(&self) -> Box<dyn UsageObserver> {
            Box::new(Quiet)
        }
        fn models_request(&self) -> Option<(Method, &'static str)> {
            Some((Method::GET, "/models"))
        }
        fn parse_models(&self, _: &[u8]) -> Vec<ModelSuggestion> {
            vec![]
        }
    }

    #[test]
    fn only_the_requests_a_protocol_names_pass_through_and_the_query_is_no_part_of_the_path() {
        let p = Plain;
        assert!(p.allowed(&Method::POST, "/responses"));
        assert!(!p.allowed(&Method::POST, "/responses/compact"));
        assert!(p.allowed(&Method::GET, "/models"));
        assert!(p.allowed(&Method::GET, "/models?limit=5"));
        assert!(!p.allowed(&Method::GET, "/responses"), "the method counts");
        assert!(!p.allowed(&Method::POST, "/responses/resp_1/cancel"));
        assert!(!p.allowed(&Method::POST, "/chat/completions"));
        assert!(!p.allowed(&Method::DELETE, "/models"));
        assert!(is_allowed(
            &ANTHROPIC_MESSAGES_REQUESTS,
            &Method::POST,
            "/v1/messages/count_tokens"
        ));
        assert!(!is_allowed(
            &ANTHROPIC_MESSAGES_REQUESTS,
            &Method::POST,
            "/v1/complete"
        ));
        assert!(is_allowed(
            &OPENAI_CHAT_REQUESTS,
            &Method::POST,
            "/chat/completions"
        ));
        assert!(is_allowed(
            &OPENAI_EMBEDDINGS_REQUESTS,
            &Method::POST,
            "/embeddings"
        ));
        assert!(!is_allowed(
            &OPENAI_EMBEDDINGS_REQUESTS,
            &Method::POST,
            "/chat/completions"
        ));
        assert!(
            Plain.embedding_request("m", &["x".into()], None).is_none(),
            "a protocol makes embeddings only where it says so"
        );
    }

    #[test]
    fn the_model_is_the_member_of_the_body_as_it_is_written() {
        assert_eq!(
            model_in_body(br#"{"model":"openai/gpt-6-sol:free","input":"x"}"#).as_deref(),
            Some("openai/gpt-6-sol:free")
        );
        assert_eq!(model_in_body(br#"{"input":"x"}"#), None);
        assert_eq!(model_in_body(br#"{"model":7}"#), None);
        assert_eq!(model_in_body(b"not json"), None);
    }

    #[test]
    fn a_protocol_presents_the_key_as_bearer_unless_the_endpoint_says_otherwise() {
        let p = Plain;
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer harness"));
        let options = EndpointOptions::default();
        p.authorize(
            &mut headers,
            Some(&Credential::new("sk-1")),
            &options,
            &SecretHeaders::new(),
        )
        .unwrap();
        assert_eq!(headers["authorization"], "Bearer sk-1");
        let fields: Vec<_> = p
            .endpoint_options()
            .into_iter()
            .map(|field| field.name)
            .collect();
        assert_eq!(
            fields,
            [
                "auth",
                "auth_header",
                "headers",
                "secret_headers",
                "query",
                "params",
                "output_limit_field",
                "request_options"
            ]
        );
    }

    #[test]
    fn every_protocol_has_the_key_as_it_is_and_a_refusal_is_in_its_own_shape() {
        let p = Plain;
        assert_eq!(p.credential_forms()[0].form, "api_key");
        let url = Url::parse("https://api.example.com").unwrap();
        let issued =
            block_on(p.issue_credential("api_key", &Credential::new("sk-1"), &url)).unwrap();
        assert_eq!(issued.credential.expose(), "sk-1");
        assert_eq!(
            block_on(p.issue_credential("minted", &Credential::new("sk-1"), &url)),
            Err(WireError::UnknownForm("minted".into()))
        );
        let refusal = p.deny(
            StatusCode::FORBIDDEN,
            "model_not_allowed",
            "not on the lease",
        );
        assert_eq!(refusal.status(), StatusCode::FORBIDDEN);
        assert!(String::from_utf8_lossy(refusal.body()).contains("model_not_allowed"));
    }

    #[test]
    fn undeclared_headers_are_never_inferred_from_a_compatible_body() {
        let p = Plain;
        let mut headers = HeaderMap::new();
        p.finalize_request_headers(&mut headers, br#"{"speed":"fast"}"#)
            .unwrap();
        assert!(headers.is_empty());
        assert!(p.joined_headers().is_empty());
        assert!(p.request_header_rules().is_empty());
    }

    #[test]
    fn declared_model_lists_preserve_names_and_refuse_per_candidate_overrides() {
        let lists = &[
            ModelListField {
                pointer: "/models",
                member: None,
            },
            ModelListField {
                pointer: "/fallbacks",
                member: Some("model"),
            },
        ];
        assert_eq!(model_candidates(Some("first".into()), br#"{"models":["first","vendor/second"],"fallbacks":[{"model":"third"}],"metadata":{"model":"unrelated"}}"#, lists).unwrap(), ["first", "vendor/second", "third"]);
        assert_eq!(
            model_candidates(None, br#"{"models":["vendor/first","second"]}"#, lists).unwrap(),
            ["vendor/first", "second"]
        );
        for body in [
            br#"{"models":null}"#.as_slice(),
            br#"{"models":"other"}"#,
            br#"{"models":["first",false]}"#,
            br#"{"models":[" "]}"#,
            br#"{"fallbacks":[{"model":"second","speed":"fast"}]}"#,
            br#"{"fallbacks":[{"model":null}]}"#,
        ] {
            assert!(model_candidates(Some("first".into()), body, lists).is_err());
        }
        assert!(Plain.model_lists().is_empty());
        assert_eq!(
            Plain
                .model_candidates(
                    "/responses",
                    br#"{"model":"first","models":["an undeclared field"]}"#
                )
                .unwrap(),
            ["first"]
        );
    }
}

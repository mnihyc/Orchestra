//! ProtocolTranslator seam, declared conversion losses, and the endpoint and credential fields its upstream needs; for built-in and external implementations.
//!
//! Direct requests are plain relays: a request passes through in the protocol it came in. A **translator**
//! converts requests of one wire protocol (`from`: the one the harness speaks) into another (`to`: the one
//! the upstream speaks) and the response stream back, chunk by chunk. The shipped Responses / Messages extension implements both directions. Additional translators may be
//! external implementations registered beside the provider runtime.
//!
//! **Who presents the credential.** Where `to` is a protocol that has an implementation, the translator
//! leaves the credential alone and that implementation presents it as the endpoint's options say. Where
//! `to` names a format nothing else implements, the translator is the whole of that side: its descriptor
//! declares the endpoint options and the credential fields, and it presents the credential itself
//! (`authorized: true`) ([`presenter`]). For an external translator the credential travels in that one
//! request, over the plugin's local channel inside the provider's runtime, where the key already is.
//!
//! **What a provider serves.** A provider's upstream has one or more endpoints, each speaking one protocol.
//! The provider can serve those protocols plus every protocol for which a registered translator ends at one
//! of them ([`served_protocols`]); for a request, the forwarder uses the endpoint that speaks its protocol,
//! or translates to one that exists ([`route`]). A translator's descriptor states its losses, and the
//! binding dialog shows them when a pairing relies on one.
//!
//! On the local plugin channel `translate_request` receives `path`, a UTF-8
//! text `body` and `upstream`; it returns the [`TranslatedRequest`] fields.
//! `response_translator` receives no parameters and returns `{"converter":"id"}`.
//! `translate_chunk` receives `converter` and a text `chunk`, returning
//! `{"chunk":"converted text"}`; `finish_response` receives `converter`
//! and returns the last text chunk. Converter identities belong to one response.
//! The bridge retains an incomplete UTF-8 suffix across byte chunks, so each
//! plugin call sees whole characters. Finish flushes retained output and frees
//! the converter; dropping a response also finishes it. The upstream credential
//! appears only on the runtime's local channel when this translator presents it.

use std::collections::BTreeMap;

use bytes::Bytes;
use orch_plugin_contract::Extensions;
use orch_types::{Field, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::credentials::Credential;

pub use orch_provider_contract::models::{ConversionLosses, ProtocolTranslation};

/// What a translator declares of itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TranslatorDescriptor {
    /// The protocol the harness speaks.
    pub from: String,
    /// The protocol the upstream speaks.
    pub to: String,
    /// What it is called.
    pub title: String,
    /// What it does, in a sentence.
    #[serde(default)]
    pub summary: String,
    /// What is preserved, approximated and dropped.
    #[serde(default)]
    pub losses: ConversionLosses,
    /// What an endpoint on the `to` side may configure; used when nothing else implements `to`.
    #[serde(default)]
    pub endpoint_options: Vec<Field>,
    /// What the credential of such an upstream consists of, beside or instead of a key.
    #[serde(default)]
    pub credential_fields: Vec<Field>,
    /// Members that belong to the implementation, kept and never interpreted.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// The endpoint a translator translates to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpstreamSide {
    /// The endpoint's base address.
    pub base_url: String,
    /// Its options as the `to` side declares them.
    #[serde(default)]
    pub options: Json,
    /// Captured output default, after the caller admitted this translated pairing.
    pub output_tokens: i64,
    /// Opaque stable session cache key; contains neither the original session identity nor a person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The selected model's captured option declarations and reported output limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<orch_provider_contract::models::ModelParameters>,
    /// The key, when the translator presents it itself.
    #[serde(default)]
    pub credential: Option<Credential>,
}

/// Bytes that are text, written as a string.
mod text {
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(bytes: &Bytes, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(std::str::from_utf8(bytes).map_err(serde::ser::Error::custom)?)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Bytes, D::Error> {
        let text = String::deserialize(deserializer).map_err(D::Error::custom)?;
        Ok(Bytes::from(text))
    }
}

/// A request as it is to be made of the upstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TranslatedRequest {
    /// The method.
    pub method: String,
    /// The path below the upstream's base.
    pub path: String,
    /// The query parameters.
    #[serde(default)]
    pub query: BTreeMap<String, String>,
    /// The headers to send.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// The body; text, as the model APIs' bodies are.
    #[serde(with = "text")]
    #[schemars(with = "String")]
    pub body: Bytes,
    /// Whether the translator presented the credential itself.
    pub authorized: bool,
}

/// Which side presents the credential on the upstream request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Presenter {
    /// The wire protocol that implements `to`, as the endpoint's options say.
    WireProtocol,
    /// The translator, because nothing else implements `to`.
    Translator,
}

/// Who presents the credential for a translator that ends at `to`, given the protocols that have an
/// implementation (`implemented`): the implementation, where there is one, and otherwise the translator.
#[must_use]
pub fn presenter(to: &str, implemented: &[&str]) -> Presenter {
    if implemented.contains(&to) {
        Presenter::WireProtocol
    } else {
        Presenter::Translator
    }
}

/// Why a request could not be translated.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TranslateError {
    /// The request is not a request of the `from` protocol.
    #[error("the request is malformed: {0}")]
    Malformed(String),
    /// The request holds something with no equivalent on the other side that the translator will not drop.
    #[error("the request cannot be converted: {0}")]
    Unsupported(String),
    /// The translator presented the credential where a wire protocol does it, or left it where nothing
    /// else will.
    #[error("the credential is presented by {0}")]
    Credential(&'static str),
    /// The translator failed in a way that is its own.
    #[error("internal error: {0}")]
    Internal(String),
}

impl TranslatedRequest {
    /// Checks that the translator handled the credential as the protocol on the `to` side requires:
    /// `authorized` where the translator is the whole of that side, and not otherwise.
    pub fn check_for(&self, presenter: Presenter) -> Result<(), TranslateError> {
        match (presenter, self.authorized) {
            (Presenter::Translator, false) => Err(TranslateError::Credential(
                "the translator, and it did not present it",
            )),
            (Presenter::WireProtocol, true) => Err(TranslateError::Credential(
                "the wire protocol of the endpoint, and the translator did too",
            )),
            _ => Ok(()),
        }
    }
}

/// Converts the response stream of the upstream back into the protocol the harness speaks, chunk by chunk.
pub trait ResponseTranslator: Send {
    /// Converts the next piece of the response. It may return nothing yet, and it may return more than it
    /// was given; it must not hold the stream longer than the conversion needs.
    fn translate_chunk(&mut self, chunk: Bytes) -> Result<Bytes, TranslateError>;

    /// The response has ended: whatever the conversion still holds, and the closing of the stream.
    fn finish(&mut self) -> Result<Bytes, TranslateError>;
}

/// One implementation per pair of protocols, an external implementation (`kind = "translator"`).
pub trait ProtocolTranslator: Send + Sync + 'static {
    /// The protocol the harness speaks.
    fn from(&self) -> &'static str;

    /// The protocol the upstream speaks.
    fn to(&self) -> &'static str;

    /// What it preserves, approximates and drops; and the endpoint options and credential fields its
    /// upstream needs.
    fn describe(&self) -> TranslatorDescriptor;

    /// Converts a request of the `from` protocol, to `path` below the base, for the endpoint `upstream`.
    fn translate_request(
        &self,
        path: &str,
        body: Bytes,
        upstream: &UpstreamSide,
    ) -> Result<TranslatedRequest, TranslateError>;

    /// A converter of the response stream, for one response.
    fn response_translator(&self) -> Box<dyn ResponseTranslator>;

    /// One answer converter with this request's tool and route context. The default is stateless.
    fn response_for(
        &self,
        _path: &str,
        _body: &[u8],
        _upstream: &UpstreamSide,
    ) -> Result<Box<dyn ResponseTranslator>, TranslateError> {
        Ok(self.response_translator())
    }
}

/// The ends of a registered translator: the protocol the harness speaks and the one the upstream speaks.
pub type Ends<'a> = (&'a str, &'a str);

/// The protocols a provider can serve: those of its endpoints, in their order, and then every protocol for
/// which a registered translator ends at one of them. With no translators registered, exactly the
/// endpoints' protocols.
#[must_use]
pub fn served_protocols(endpoints: &[&str], translators: &[Ends<'_>]) -> Vec<String> {
    let mut served: Vec<String> = Vec::new();
    let mut add = |protocol: &str| {
        if !served.iter().any(|known| known == protocol) {
            served.push(protocol.to_owned());
        }
    };
    for protocol in endpoints {
        add(protocol);
    }
    for (from, to) in translators {
        if endpoints.contains(to) {
            add(from);
        }
    }
    served
}

/// What the forwarder does with a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// An endpoint speaks the request's protocol: the request passes through to it.
    Direct,
    /// A translator converts it to the protocol `to`, which an endpoint speaks.
    Translate {
        /// The protocol the endpoint speaks.
        to: String,
    },
    /// Nothing serves the protocol: the request is refused.
    Unserved,
}

/// How a request in protocol `request` is served: by the endpoint that speaks it, or else by the first
/// translator (in registration order) that converts it to a protocol an endpoint speaks.
#[must_use]
pub fn route(request: &str, endpoints: &[&str], translators: &[Ends<'_>]) -> Route {
    if endpoints.contains(&request) {
        return Route::Direct;
    }
    translators
        .iter()
        .find(|(from, to)| *from == request && endpoints.contains(to))
        .map_or(Route::Unserved, |(_, to)| Route::Translate {
            to: (*to).to_owned(),
        })
}

/// The built-in format conversions, considered only after direct endpoint matches.
pub const BUILTIN_ENDS: [Ends<'static>; 2] = [
    (
        crate::protocol::OPENAI_RESPONSES,
        crate::protocol::ANTHROPIC_MESSAGES,
    ),
    (
        crate::protocol::ANTHROPIC_MESSAGES,
        crate::protocol::OPENAI_RESPONSES,
    ),
];

/// Public fidelity declarations shared by the converter and the choices that offer it.
#[must_use]
pub fn builtin_descriptors() -> [TranslatorDescriptor; 2] {
    [
        builtin_descriptor(BUILTIN_ENDS[0].0, BUILTIN_ENDS[0].1, true),
        builtin_descriptor(BUILTIN_ENDS[1].0, BUILTIN_ENDS[1].1, false),
    ]
}

/// A conversion's public facts, or none for a direct or unserved interface.
#[must_use]
pub fn builtin_translation(from: &str, endpoints: &[&str]) -> Option<ProtocolTranslation> {
    if endpoints.contains(&from) {
        return None;
    }
    let descriptor = builtin_descriptors()
        .into_iter()
        .find(|item| item.from == from && endpoints.contains(&item.to.as_str()))?;
    Some(ProtocolTranslation {
        from: descriptor.from,
        to: descriptor.to,
        title: descriptor.title,
        losses: descriptor.losses,
    })
}

fn builtin_descriptor(from: &str, to: &str, responses: bool) -> TranslatorDescriptor {
    TranslatorDescriptor {
        from: from.into(),
        to: to.into(),
        title: "Responses / Messages".into(),
        summary: "Stream text, images and client tools across the two model interfaces.".into(),
        losses: ConversionLosses {
            preserved: vec![
                "Text, image inputs, tool identifiers and upstream token usage.".into(),
                "Signed reasoning is carried opaquely and restored on the next turn.".into(),
                "Rate limits, overload, output limits and interrupted streams retain their meaning.".into(),
            ],
            approximated: if responses { vec![
                    "Visible reasoning summaries survive an upstream change; incompatible opaque state is omitted.".into(),
                    "Later system instructions become instructions in the following user turn.".into(),
                    "Custom tool grammars become a single string argument; the upstream does not enforce the grammar.".into(),
                    "Prefix caching uses system and newest-turn cache breakpoints.".into(),
                    "Unpaired tool notifications become user input with their source name.".into(),
                    "Messages from other agents become user input carrying author and recipient.".into(),
                    "Reasoning effort uses the offering's declared thinking controls.".into(),
                    "Compaction produces a text summary carried as opaque conversation state.".into(),
                ] } else { vec![
                    "Visible reasoning summaries survive an upstream change; incompatible opaque state is omitted.".into(),
                    "Images in a tool result follow the tool result as user image inputs.".into(),
                    "Explicit cache breakpoints become a stable session cache key when one is provided.".into(),
                    "Thinking budgets use the offering's declared reasoning effort.".into(),
                ] },
            dropped: if responses { vec!["Vendor server tools are not translated; use Orch's capability routes.".into()] } else { vec![
                    "Stop sequences and sampling controls without an outgoing equivalent.".into(),
                    "Unrecognized beta headers are not forwarded across interfaces.".into(),
                ] },
        },
        endpoint_options: Vec::new(),
        credential_fields: Vec::new(),
        extensions: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A translator between two test protocols, `alpha` (what the harness speaks) and `beta`: it turns the
    /// word `ping` of a request into `pong`, and upper-cases the response.
    struct AlphaToBeta;

    struct Shout;
    impl ResponseTranslator for Shout {
        fn translate_chunk(&mut self, chunk: Bytes) -> Result<Bytes, TranslateError> {
            Ok(Bytes::from(String::from_utf8_lossy(&chunk).to_uppercase()))
        }
        fn finish(&mut self) -> Result<Bytes, TranslateError> {
            Ok(Bytes::from_static(b"[done]"))
        }
    }

    impl ProtocolTranslator for AlphaToBeta {
        fn from(&self) -> &'static str {
            "alpha"
        }
        fn to(&self) -> &'static str {
            "beta"
        }
        fn describe(&self) -> TranslatorDescriptor {
            TranslatorDescriptor {
                from: "alpha".into(),
                to: "beta".into(),
                title: "Alpha to Beta".into(),
                summary: "Speaks Beta to an Alpha harness.".into(),
                losses: ConversionLosses {
                    preserved: vec!["messages".into()],
                    approximated: vec![],
                    dropped: vec!["images".into()],
                },
                endpoint_options: vec![],
                credential_fields: vec![],
                extensions: Extensions::new(),
            }
        }
        fn translate_request(
            &self,
            path: &str,
            body: Bytes,
            _: &UpstreamSide,
        ) -> Result<TranslatedRequest, TranslateError> {
            let text =
                std::str::from_utf8(&body).map_err(|e| TranslateError::Malformed(e.to_string()))?;
            Ok(TranslatedRequest {
                method: "POST".into(),
                path: format!("/beta{path}"),
                query: BTreeMap::new(),
                headers: BTreeMap::new(),
                body: Bytes::from(text.replace("ping", "pong")),
                authorized: false,
            })
        }
        fn response_translator(&self) -> Box<dyn ResponseTranslator> {
            Box::new(Shout)
        }
    }

    #[test]
    fn a_translator_converts_a_request_and_the_response_stream_back() {
        let t = AlphaToBeta;
        let upstream = UpstreamSide {
            base_url: "https://beta.example.com".into(),
            options: Json::Null,
            credential: None,
            model: None,
            output_tokens: 4096,
            session: None,
        };
        let request = t
            .translate_request("/chat", Bytes::from_static(b"ping ping"), &upstream)
            .unwrap();
        assert_eq!(
            (request.method.as_str(), request.path.as_str()),
            ("POST", "/beta/chat")
        );
        assert_eq!(request.body, "pong pong");
        let mut response = t.response_translator();
        assert_eq!(
            response
                .translate_chunk(Bytes::from_static(b"hi "))
                .unwrap(),
            "HI "
        );
        assert_eq!(response.finish().unwrap(), "[done]");
        assert_eq!((t.from(), t.to()), ("alpha", "beta"));
        assert!(
            t.describe().losses.has_losses(),
            "the binding dialog shows the losses"
        );
    }

    #[test]
    fn the_credential_is_presented_by_the_wire_protocol_where_there_is_one_and_by_the_translator_where_not()
     {
        assert_eq!(
            presenter(
                "openai-responses",
                &["openai-responses", "anthropic-messages"]
            ),
            Presenter::WireProtocol
        );
        assert_eq!(
            presenter("beta", &["openai-responses"]),
            Presenter::Translator
        );
        let mut request = AlphaToBeta
            .translate_request(
                "/x",
                Bytes::new(),
                &UpstreamSide {
                    base_url: String::new(),
                    options: Json::Null,
                    credential: None,
                    model: None,
                    output_tokens: 4096,
                    session: None,
                },
            )
            .unwrap();
        assert_eq!(request.check_for(Presenter::WireProtocol), Ok(()));
        assert!(matches!(
            request.check_for(Presenter::Translator),
            Err(TranslateError::Credential(_))
        ));
        request.authorized = true;
        assert_eq!(request.check_for(Presenter::Translator), Ok(()));
        assert!(matches!(
            request.check_for(Presenter::WireProtocol),
            Err(TranslateError::Credential(_))
        ));
    }

    #[test]
    fn a_provider_serves_its_endpoints_protocols_and_those_a_translator_ends_at() {
        assert_eq!(
            served_protocols(&["beta"], &[]),
            ["beta"],
            "with no translator, exactly the endpoints'"
        );
        assert_eq!(
            served_protocols(&["beta"], &[("alpha", "beta")]),
            ["beta", "alpha"]
        );
        assert_eq!(
            served_protocols(
                &["openai-responses", "anthropic-messages"],
                &[("openai-chat", "openai-responses"), ("alpha", "beta")]
            ),
            ["openai-responses", "anthropic-messages", "openai-chat"],
            "a translator that ends at no endpoint adds nothing"
        );
        assert_eq!(
            served_protocols(
                &["beta"],
                &[("beta", "beta"), ("alpha", "beta"), ("alpha", "beta")]
            ),
            ["beta", "alpha"]
        );
    }

    #[test]
    fn a_request_goes_to_the_endpoint_that_speaks_it_or_is_translated_or_refused() {
        let translators = [("alpha", "beta"), ("alpha", "gamma")];
        assert_eq!(
            route("beta", &["beta", "gamma"], &translators),
            Route::Direct
        );
        assert_eq!(
            route("alpha", &["beta"], &translators),
            Route::Translate { to: "beta".into() }
        );
        assert_eq!(
            route("alpha", &["gamma"], &translators),
            Route::Translate { to: "gamma".into() }
        );
        assert_eq!(
            route("alpha", &["beta", "gamma"], &translators),
            Route::Translate { to: "beta".into() }
        );
        assert_eq!(route("delta", &["beta"], &translators), Route::Unserved);
        assert_eq!(
            route("alpha", &["beta"], &[]),
            Route::Unserved,
            "with no translator registered nothing is translated"
        );
    }

    #[test]
    fn a_translated_request_is_text_on_the_wire_and_a_descriptor_keeps_members_of_its_own() {
        let request: TranslatedRequest = serde_json::from_value(json!({
            "method": "POST", "path": "/v1/chat", "query": { "v": "1" }, "headers": { "x-a": "b" },
            "body": "{\"model\":\"m\"}", "authorized": true
        }))
        .unwrap();
        assert_eq!(request.body, "{\"model\":\"m\"}");
        assert_eq!(
            serde_json::to_value(&request).unwrap()["body"],
            "{\"model\":\"m\"}"
        );
        let descriptor: TranslatorDescriptor = serde_json::from_value(json!({
            "from": "openai-chat", "to": "acme-chat", "title": "Chat to Acme",
            "losses": { "dropped": ["logprobs"] },
            "endpoint_options": [ { "name": "region", "label": "Region", "type": "text", "required": true, "help": null,
                                    "choices": null, "default": null, "max_bytes": null } ],
            "credential_fields": [], "x_vendor_docs": "https://docs.acme.example"
        }))
        .unwrap();
        assert_eq!(
            descriptor.extensions.get("x_vendor_docs"),
            Some(&json!("https://docs.acme.example"))
        );
        assert!(descriptor.losses.has_losses());
        let upstream: UpstreamSide = serde_json::from_value(
            json!({ "base_url": "https://acme.example", "credential": "k-1" }),
        )
        .unwrap();
        assert_eq!(upstream.credential.as_ref().unwrap().expose(), "k-1");
        assert!(!format!("{upstream:?}").contains("k-1"));
    }
}

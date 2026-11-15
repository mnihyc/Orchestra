//! Requests, signed cross-turn state and incremental answers between Responses and Messages.
//!
//! Credentials stay with the outgoing wire protocol. Each response converter belongs to one request;
//! custom tools and count/compaction answers cannot leak into another concurrent conversion.
#![forbid(unsafe_code)]

mod requests;
mod stream;

use std::collections::{BTreeMap, BTreeSet};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use orch_model_contract::protocol::{ANTHROPIC_MESSAGES, OPENAI_RESPONSES};
use orch_model_contract::translator::{
    ConversionLosses, ProtocolTranslator, ResponseTranslator, TranslateError, TranslatedRequest,
    TranslatorDescriptor, UpstreamSide,
};
use serde_json::{Value, json};

/// Responses calls made against a Messages endpoint.
#[derive(Debug, Clone, Copy)]
pub struct ResponsesToMessages;
/// Messages calls made against a Responses endpoint.
#[derive(Debug, Clone, Copy)]
pub struct MessagesToResponses;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Responses,
    Messages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Generation,
    Count,
    Compact,
}

fn malformed(error: impl std::fmt::Display) -> TranslateError {
    TranslateError::Malformed(error.to_string())
}

fn unsupported(message: impl Into<String>) -> TranslateError {
    TranslateError::Unsupported(message.into())
}

fn parse(body: &[u8]) -> Result<Value, TranslateError> {
    let value: Value = serde_json::from_slice(body).map_err(malformed)?;
    if !value.is_object() {
        return Err(malformed("a model request is a JSON object"));
    }
    Ok(value)
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn opaque(prefix: &str, value: &Value) -> String {
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(value.to_string()))
}

fn restore(prefix: &str, value: &Value) -> Result<Option<Value>, TranslateError> {
    let Some(encoded) = value.as_str().and_then(|value| value.strip_prefix(prefix)) else {
        return Ok(None);
    };
    let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(malformed)?;
    serde_json::from_slice(&bytes).map(Some).map_err(malformed)
}

fn custom_tools(request: &Value) -> BTreeSet<String> {
    request
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|tool| tool["type"] == "custom")
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect()
}

fn descriptor(from: &str, to: &str, direction: Direction) -> TranslatorDescriptor {
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
            approximated: match direction {
                Direction::Responses => vec![
                    "Later system instructions become instructions in the following user turn.".into(),
                    "Custom tool grammars become a single string argument; the upstream does not enforce the grammar.".into(),
                    "Prefix caching uses system and newest-turn cache breakpoints.".into(),
                    "Reasoning effort uses the offering's declared thinking controls.".into(),
                    "Compaction produces a text summary carried as opaque conversation state.".into(),
                ],
                Direction::Messages => vec![
                    "Images in a tool result follow the tool result as user image inputs.".into(),
                    "Explicit cache breakpoints become a stable session cache key when one is provided.".into(),
                    "Thinking budgets use the offering's declared reasoning effort.".into(),
                ],
            },
            dropped: match direction {
                Direction::Responses => vec!["Vendor server tools are not translated; use Orch's capability routes.".into()],
                Direction::Messages => vec![
                    "Stop sequences and sampling controls without an outgoing equivalent.".into(),
                    "Unrecognized beta headers are not forwarded across interfaces.".into(),
                ],
            },
        },
        endpoint_options: Vec::new(),
        credential_fields: Vec::new(),
        extensions: BTreeMap::new(),
    }
}

fn translate(
    direction: Direction,
    path: &str,
    body: Bytes,
    upstream: &UpstreamSide,
) -> Result<TranslatedRequest, TranslateError> {
    let request = parse(&body)?;
    if request
        .get("previous_response_id")
        .is_some_and(|id| !id.is_null())
        || request.get("conversation").is_some_and(|id| !id.is_null())
    {
        return Err(unsupported(
            "Pass the conversation items; upstream stored conversations cannot cross interfaces",
        ));
    }
    let path = path.split('?').next().unwrap_or(path);
    let (path, mut body) = match direction {
        Direction::Responses => match path {
            "/responses" => ("/v1/messages", requests::to_messages(&request, upstream)?),
            "/responses/compact" => {
                let mut body = requests::to_messages(&request, upstream)?;
                body["stream"] = json!(false);
                let instruction = "Summarize this conversation for continuing the same work. Preserve decisions, constraints, unfinished work, tool results and file changes. Return only the continuation summary.";
                let messages = body["messages"]
                    .as_array_mut()
                    .ok_or_else(|| malformed("messages are absent"))?;
                requests::append_message(
                    messages,
                    "user",
                    vec![json!({"type":"text","text":instruction})],
                );
                ("/v1/messages", body)
            }
            _ => {
                return Err(unsupported(
                    "This Responses path has no Messages equivalent",
                ));
            }
        },
        Direction::Messages => match path {
            "/v1/messages" => ("/responses", requests::to_responses(&request)?),
            "/v1/messages/count_tokens" => {
                let mut body = requests::to_responses(&request)?;
                if let Some(object) = body.as_object_mut() {
                    for key in [
                        "stream",
                        "store",
                        "max_output_tokens",
                        "include",
                        "reasoning",
                    ] {
                        object.remove(key);
                    }
                }
                ("/responses/input_tokens", body)
            }
            _ => {
                return Err(unsupported(
                    "This Messages path has no Responses equivalent",
                ));
            }
        },
    };
    // A translated response carries enough state to continue without an upstream conversation.
    if direction == Direction::Messages && path == "/responses" {
        body["store"] = json!(false);
        body["include"] = json!(["reasoning.encrypted_content"]);
    }
    Ok(TranslatedRequest {
        method: "POST".into(),
        path: path.into(),
        query: BTreeMap::new(),
        headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
        body: Bytes::from(body.to_string()),
        authorized: false,
    })
}

macro_rules! translator {
    ($name:ident, $from:expr, $to:expr, $direction:expr) => {
        impl ProtocolTranslator for $name {
            fn from(&self) -> &'static str {
                $from
            }
            fn to(&self) -> &'static str {
                $to
            }
            fn describe(&self) -> TranslatorDescriptor {
                descriptor($from, $to, $direction)
            }
            fn translate_request(
                &self,
                path: &str,
                body: Bytes,
                upstream: &UpstreamSide,
            ) -> Result<TranslatedRequest, TranslateError> {
                translate($direction, path, body, upstream)
            }
            fn response_translator(&self) -> Box<dyn ResponseTranslator> {
                Box::new(stream::Converter::new(
                    $direction,
                    Answer::Generation,
                    BTreeSet::new(),
                ))
            }
            fn response_for(
                &self,
                path: &str,
                body: &[u8],
                _upstream: &UpstreamSide,
            ) -> Result<Box<dyn ResponseTranslator>, TranslateError> {
                let request = parse(body)?;
                let answer = match path.split('?').next().unwrap_or(path) {
                    "/responses/compact" => Answer::Compact,
                    "/v1/messages/count_tokens" => Answer::Count,
                    _ => Answer::Generation,
                };
                Ok(Box::new(stream::Converter::new(
                    $direction,
                    answer,
                    custom_tools(&request),
                )))
            }
        }
    };
}

translator!(
    ResponsesToMessages,
    OPENAI_RESPONSES,
    ANTHROPIC_MESSAGES,
    Direction::Responses
);
translator!(
    MessagesToResponses,
    ANTHROPIC_MESSAGES,
    OPENAI_RESPONSES,
    Direction::Messages
);

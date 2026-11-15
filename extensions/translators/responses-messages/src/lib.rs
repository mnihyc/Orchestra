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
    ProtocolTranslator, ResponseTranslator, TranslateError, TranslatedRequest,
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
    CompactStream,
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

fn compaction_trigger(request: &Value) -> bool {
    request
        .get("input")
        .and_then(Value::as_array)
        .and_then(|items| items.last())
        .is_some_and(|item| item["type"] == "compaction_trigger")
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
    let compact = path == "/responses/compact" || compaction_trigger(&request);
    let (path, mut body) = match direction {
        Direction::Responses => match path {
            "/responses" | "/responses/compact" => {
                let mut body = requests::to_messages(&request, upstream)?;
                if compact {
                    body["stream"] = if path == "/responses/compact" {
                        json!(false)
                    } else {
                        request.get("stream").cloned().unwrap_or(json!(false))
                    };
                    let instruction = "Summarize this conversation for continuing the same work. Preserve decisions, constraints, unfinished work, tool results and file changes. Return only the continuation summary.";
                    let messages = body["messages"]
                        .as_array_mut()
                        .ok_or_else(|| malformed("messages are absent"))?;
                    requests::append_message(
                        messages,
                        "user",
                        vec![json!({"type":"text","text":instruction})],
                    );
                    if let Some(object) = body.as_object_mut() {
                        object.remove("tools");
                        object.remove("tool_choice");
                    }
                }
                ("/v1/messages", body)
            }
            _ => {
                return Err(unsupported(
                    "This Responses path has no Messages equivalent",
                ));
            }
        },
        Direction::Messages => match path {
            "/v1/messages" => ("/responses", requests::to_responses(&request, upstream)?),
            "/v1/messages/count_tokens" => {
                let mut body = requests::to_responses(&request, upstream)?;
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
                let [responses, messages] = orch_model_contract::translator::builtin_descriptors();
                match $direction {
                    Direction::Responses => responses,
                    Direction::Messages => messages,
                }
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
                    "/responses" if compaction_trigger(&request) => Answer::CompactStream,
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

//! Requests, replies, progress, events and cancellation.
//!
//! The plugin transport is JSON Lines: each line is one JSON object, in either direction.
//!
//! | Direction | Shape | Meaning |
//! |-----------|-------|---------|
//! | host → plugin | `{ "id": 1, "method": "<name>", "params": { … } }` | A request |
//! | plugin → host | `{ "id": 1, "result": { … } }` or `{ "id": 1, "error": { "code": "…", "message": "…", "retryable": false } }` | Its answer |
//! | plugin → host | `{ "progress": 1, "params": { "message": "…", "done": 3, "total": 10 } }` | Progress of request 1, any number of times before its answer |
//! | host → plugin | `{ "method": "cancel", "params": { "id": 1 } }` | Stop working on request 1; it is then answered with the error `cancelled` |
//! | plugin → host | `{ "event": "<name>", "params": { … } }` | An unsolicited event |
//! | host → plugin | `{ "id": 2, "method": "ping" }` | Answered with an empty result |
//! | host → plugin | `{ "method": "shutdown" }` | Finish and exit |
//!
//! Requests may overlap; answers are matched by `id`. Binary content is never sent inline: plugins that move
//! bytes are given paths or loopback URLs. Both sides ignore members they do not know, and a method a side
//! does not know is answered with the error `unsupported`, never by failing.
//!
//! A line is read into [`ToPlugin`] (what a host writes) or [`FromPlugin`] (what a plugin writes) with
//! `parse`, and written with `to_line`.

use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, json};

/// The id of a request: the host numbers them, and the first exchange, the handshake, is 0.
pub type RequestId = i64;

/// The id of the handshake.
pub const HELLO_ID: RequestId = 0;

/// The methods every implementation has, whatever its kind.
pub mod method {
    /// The first exchange.
    pub const HELLO: &str = "hello";
    /// Asked after a time without traffic; answered with an empty result.
    pub const PING: &str = "ping";
    /// Stop working on a request.
    pub const CANCEL: &str = "cancel";
    /// Finish and exit.
    pub const SHUTDOWN: &str = "shutdown";
}

/// The codes of the errors the transport itself gives.
pub mod code {
    /// The request was cancelled.
    pub const CANCELLED: &str = "cancelled";
    /// The method is not known to the side that was asked.
    pub const UNSUPPORTED: &str = "unsupported";
    /// No answer came within the method's deadline.
    pub const TIMEOUT: &str = "timeout";
    /// The side asked failed in a way that is its own.
    pub const INTERNAL: &str = "internal";
}

/// Why a request failed, as the plugin states it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PluginError {
    /// A code: the ones of [`code`], and for an adapter those of the link's command errors.
    pub code: String,
    /// What happened, in a sentence.
    pub message: String,
    /// Whether trying again may help.
    #[serde(default)]
    pub retryable: bool,
}

impl PluginError {
    /// An error of the code `code`.
    #[must_use]
    pub fn new(code: &str, message: &str, retryable: bool) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
            retryable,
        }
    }

    /// The answer to a method that is not known.
    #[must_use]
    pub fn unsupported(method: &str) -> Self {
        Self::new(
            code::UNSUPPORTED,
            &format!("unknown method {method:?}"),
            false,
        )
    }

    /// The answer to a request that was cancelled.
    #[must_use]
    pub fn cancelled() -> Self {
        Self::new(code::CANCELLED, "cancelled", false)
    }

    /// What a request comes to when no answer came in time: retryable.
    #[must_use]
    pub fn timeout(method: &str) -> Self {
        Self::new(
            code::TIMEOUT,
            &format!("{method} was not answered in time"),
            true,
        )
    }
}

/// A request of the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Request {
    /// Its id.
    pub id: RequestId,
    /// The method: the name of a trait method in `snake_case`, and for adapters the command names.
    pub method: String,
    /// Its parameters; absent for a method that takes none.
    #[serde(default, skip_serializing_if = "Json::is_null")]
    pub params: Json,
}

/// How far a request has come.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProgressParams {
    /// What is being done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// How much is done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<i64>,
    /// How much there is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
}

/// Progress of a request, any number of times before its answer; shown where the operation is shown, and it
/// does not extend the request's deadline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Progress {
    /// The request it is about.
    pub progress: RequestId,
    /// How far it has come.
    #[serde(default)]
    pub params: ProgressParams,
}

/// An unsolicited event of a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Event {
    /// The event's name (for adapters, the kinds of the session events and of the other reports).
    pub event: String,
    /// Its parameters.
    #[serde(default)]
    pub params: Json,
}

/// The answer to a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// The request it answers.
    pub id: RequestId,
    /// What it came to.
    pub outcome: Result<Json, PluginError>,
}

impl Reply {
    /// The answer `result` to request `id`.
    #[must_use]
    pub fn ok(id: RequestId, result: Json) -> Self {
        Self {
            id,
            outcome: Ok(result),
        }
    }

    /// The answer to a request that has nothing to say: an empty result.
    #[must_use]
    pub fn empty(id: RequestId) -> Self {
        Self::ok(id, json!({}))
    }

    /// A failure of request `id`.
    #[must_use]
    pub fn failed(id: RequestId, error: PluginError) -> Self {
        Self {
            id,
            outcome: Err(error),
        }
    }
}

/// A notification of the host: a line without an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// The method.
    pub method: String,
    /// Its parameters.
    pub params: Json,
}

/// What a host writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToPlugin {
    /// A request, to be answered.
    Request(Request),
    /// Stop working on request `id`; it is then answered with the error `cancelled`.
    Cancel {
        /// The request.
        id: RequestId,
    },
    /// Finish and exit.
    Shutdown,
    /// A notification of a method this crate does not know; ignored.
    Other(Notification),
}

/// What a plugin writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FromPlugin {
    /// The answer to a request.
    Reply(Reply),
    /// Progress of a request.
    Progress(Progress),
    /// An event.
    Event(Event),
}

/// Why a line is not a message.
#[derive(Debug, thiserror::Error)]
pub enum LineError {
    /// It is not JSON.
    #[error("the line is not JSON: {0}")]
    NotJson(#[from] serde_json::Error),
    /// It is JSON and not an object.
    #[error("the line is not a JSON object")]
    NotAnObject,
    /// It is an object that is none of the messages of that direction.
    #[error("the line is none of the messages: {0}")]
    NotAMessage(&'static str),
    /// A member is of the wrong shape.
    #[error("the member {member:?} is not of the right shape")]
    Member {
        /// The member.
        member: &'static str,
    },
}

fn object(line: &str) -> Result<Map<String, Json>, LineError> {
    match serde_json::from_str::<Json>(line.trim())? {
        Json::Object(members) => Ok(members),
        _ => Err(LineError::NotAnObject),
    }
}

fn id_member(members: &Map<String, Json>, member: &'static str) -> Result<RequestId, LineError> {
    members
        .get(member)
        .and_then(Json::as_i64)
        .ok_or(LineError::Member { member })
}

impl ToPlugin {
    /// Reads a line the host wrote.
    pub fn parse(line: &str) -> Result<Self, LineError> {
        let members = object(line)?;
        let Some(method) = members.get("method").and_then(Json::as_str) else {
            return Err(LineError::NotAMessage("a host writes a method"));
        };
        let params = members.get("params").cloned().unwrap_or(Json::Null);
        if members.contains_key("id") {
            return Ok(Self::Request(Request {
                id: id_member(&members, "id")?,
                method: method.to_owned(),
                params,
            }));
        }
        match method {
            method::CANCEL => Ok(Self::Cancel {
                id: params
                    .get("id")
                    .and_then(Json::as_i64)
                    .ok_or(LineError::Member { member: "params" })?,
            }),
            method::SHUTDOWN => Ok(Self::Shutdown),
            other => Ok(Self::Other(Notification {
                method: other.to_owned(),
                params,
            })),
        }
    }

    /// The line to write: one JSON object, no newline.
    #[must_use]
    pub fn to_line(&self) -> String {
        let value = match self {
            Self::Request(request) => serde_json::to_value(request).unwrap_or(Json::Null),
            Self::Cancel { id } => json!({ "method": method::CANCEL, "params": { "id": id } }),
            Self::Shutdown => json!({ "method": method::SHUTDOWN }),
            Self::Other(notification) => {
                json!({ "method": notification.method, "params": notification.params })
            }
        };
        value.to_string()
    }

    /// A request for the method `method`.
    #[must_use]
    pub fn request(id: RequestId, method: &str, params: Json) -> Self {
        Self::Request(Request {
            id,
            method: method.to_owned(),
            params,
        })
    }
}

impl FromPlugin {
    /// Reads a line the plugin wrote.
    pub fn parse(line: &str) -> Result<Self, LineError> {
        let members = object(line)?;
        if members.contains_key("progress") {
            return Ok(Self::Progress(Progress {
                progress: id_member(&members, "progress")?,
                params: members
                    .get("params")
                    .map(|params| serde_json::from_value(params.clone()))
                    .transpose()
                    .map_err(|_| LineError::Member { member: "params" })?
                    .unwrap_or_default(),
            }));
        }
        if let Some(event) = members.get("event") {
            return Ok(Self::Event(Event {
                event: event
                    .as_str()
                    .ok_or(LineError::Member { member: "event" })?
                    .to_owned(),
                params: members.get("params").cloned().unwrap_or(Json::Null),
            }));
        }
        if members.contains_key("id") {
            let id = id_member(&members, "id")?;
            if let Some(error) = members.get("error") {
                let error = serde_json::from_value(error.clone())
                    .map_err(|_| LineError::Member { member: "error" })?;
                return Ok(Self::Reply(Reply::failed(id, error)));
            }
            if let Some(result) = members.get("result") {
                return Ok(Self::Reply(Reply::ok(id, result.clone())));
            }
            return Err(LineError::NotAMessage("an answer has a result or an error"));
        }
        Err(LineError::NotAMessage(
            "a plugin writes an answer, progress or an event",
        ))
    }

    /// The line to write: one JSON object, no newline.
    #[must_use]
    pub fn to_line(&self) -> String {
        let value = match self {
            Self::Reply(Reply {
                id,
                outcome: Ok(result),
            }) => json!({ "id": id, "result": result }),
            Self::Reply(Reply {
                id,
                outcome: Err(error),
            }) => {
                json!({ "id": id, "error": serde_json::to_value(error).unwrap_or(Json::Null) })
            }
            Self::Progress(progress) => serde_json::to_value(progress).unwrap_or(Json::Null),
            Self::Event(event) => serde_json::to_value(event).unwrap_or(Json::Null),
        };
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_host_writes_is_read_back_line_for_line() {
        let lines = [
            r#"{"id":1,"method":"create","params":{"name":"x"}}"#,
            r#"{"id":2,"method":"ping"}"#,
            r#"{"method":"cancel","params":{"id":1}}"#,
            r#"{"method":"shutdown"}"#,
        ];
        let read: Vec<_> = lines
            .iter()
            .map(|line| ToPlugin::parse(line).unwrap())
            .collect();
        assert_eq!(
            read[0],
            ToPlugin::request(1, "create", json!({ "name": "x" }))
        );
        assert_eq!(read[1], ToPlugin::request(2, "ping", Json::Null));
        assert_eq!(read[2], ToPlugin::Cancel { id: 1 });
        assert_eq!(read[3], ToPlugin::Shutdown);
        for (line, message) in lines.iter().zip(&read) {
            assert_eq!(
                serde_json::from_str::<Json>(&message.to_line()).unwrap(),
                serde_json::from_str::<Json>(line).unwrap()
            );
        }
        assert!(
            !ToPlugin::request(2, "ping", Json::Null)
                .to_line()
                .contains("params")
        );
    }

    #[test]
    fn what_a_plugin_writes_is_an_answer_progress_or_an_event() {
        let ok = FromPlugin::parse(r#"{"id":1,"result":{"unit":"u1"}}"#).unwrap();
        assert_eq!(ok, FromPlugin::Reply(Reply::ok(1, json!({ "unit": "u1" }))));
        let failed = FromPlugin::parse(
            r#"{"id":1,"error":{"code":"timeout","message":"slow","retryable":true}}"#,
        )
        .unwrap();
        assert_eq!(
            failed,
            FromPlugin::Reply(Reply::failed(1, PluginError::new("timeout", "slow", true)))
        );
        let progress = FromPlugin::parse(
            r#"{"progress":1,"params":{"message":"pulling","done":3,"total":10}}"#,
        )
        .unwrap();
        assert_eq!(
            progress,
            FromPlugin::Progress(Progress {
                progress: 1,
                params: ProgressParams {
                    message: Some("pulling".into()),
                    done: Some(3),
                    total: Some(10)
                }
            })
        );
        let event = FromPlugin::parse(r#"{"event":"item.delta","params":{"data":"x"}}"#).unwrap();
        assert_eq!(
            event,
            FromPlugin::Event(Event {
                event: "item.delta".into(),
                params: json!({ "data": "x" })
            })
        );
        for message in [ok, failed, progress, event] {
            assert_eq!(FromPlugin::parse(&message.to_line()).unwrap(), message);
        }
    }

    #[test]
    fn an_error_that_does_not_say_whether_to_try_again_is_not_retryable() {
        let reply =
            FromPlugin::parse(r#"{"id":4,"error":{"code":"unsupported","message":"no"}}"#).unwrap();
        let FromPlugin::Reply(Reply {
            outcome: Err(error),
            ..
        }) = reply
        else {
            panic!("an error");
        };
        assert!(!error.retryable);
        assert_eq!(PluginError::unsupported("frobnicate").code, "unsupported");
        assert_eq!(PluginError::cancelled().code, "cancelled");
        assert!(PluginError::timeout("create").retryable);
    }

    #[test]
    fn members_that_are_not_known_are_ignored_and_a_notification_that_is_not_known_is_kept_apart() {
        let request = ToPlugin::parse(
            r#"{"id":7,"method":"create","params":{},"deadline":"soon","x_trace":1}"#,
        )
        .unwrap();
        assert_eq!(request, ToPlugin::request(7, "create", json!({})));
        let other = ToPlugin::parse(r#"{"method":"reload","params":{"a":1}}"#).unwrap();
        assert_eq!(
            other,
            ToPlugin::Other(Notification {
                method: "reload".into(),
                params: json!({ "a": 1 })
            })
        );
        let answer = FromPlugin::parse(r#"{"id":1,"result":{},"served_by":"p1"}"#).unwrap();
        assert_eq!(answer, FromPlugin::Reply(Reply::empty(1)));
    }

    #[test]
    fn a_line_that_is_none_of_the_messages_is_refused_with_the_reason() {
        for bad in ["not json", "[1,2]", "42"] {
            assert!(ToPlugin::parse(bad).is_err(), "{bad}");
            assert!(FromPlugin::parse(bad).is_err(), "{bad}");
        }
        assert!(matches!(
            ToPlugin::parse(r#"{"id":1}"#),
            Err(LineError::NotAMessage(_))
        ));
        assert!(matches!(
            ToPlugin::parse(r#"{"method":"cancel","params":{}}"#),
            Err(LineError::Member { member: "params" })
        ));
        assert!(matches!(
            ToPlugin::parse(r#"{"id":"one","method":"x"}"#),
            Err(LineError::Member { member: "id" })
        ));
        assert!(matches!(
            FromPlugin::parse(r#"{"id":1}"#),
            Err(LineError::NotAMessage(_))
        ));
        assert!(matches!(
            FromPlugin::parse(r#"{"hello":1}"#),
            Err(LineError::NotAMessage(_))
        ));
        assert!(matches!(
            FromPlugin::parse(r#"{"event":7}"#),
            Err(LineError::Member { member: "event" })
        ));
    }
}

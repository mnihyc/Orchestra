//! JSON lines in and out of the harness's non-interactive mode, parsed tolerantly.
//!
//! The harness writes one JSON object on each line of its standard output and reads the same from its
//! standard input. What it writes is a family of messages told apart by `type` (and by `subtype` for the
//! system ones) that grows with every release, so a [`Frame`] keeps the object whole and offers the few
//! members this adapter reads; a member or a kind it does not know is never an error. What the adapter
//! writes is a user message or a control request or response, built here so that the shape of every line
//! is in one place.

use orch_types::Json;
use serde_json::json;

/// What a line from the harness is, by its `type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameKind {
    /// Something the harness says about itself: `init`, `status`, `compact_boundary`, a task, a retry.
    System,
    /// A message of the conversation that is not the model's: a tool result, or the echo of an input.
    User,
    /// A complete block of what the model said.
    Assistant,
    /// A piece of a message the model is still writing.
    StreamEvent,
    /// The end of a turn.
    Result,
    /// The harness asks the host for something (a permission, a hook, an answer).
    ControlRequest,
    /// The harness answers a request of the host.
    ControlResponse,
    /// The harness withdraws a request it made.
    ControlCancelRequest,
    /// A line that only shows the harness is alive.
    KeepAlive,
    /// Any other kind, by its name.
    Other(String),
}

/// A line of the harness's output that is not a JSON object.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the harness wrote a line that is not a JSON object: {0}")]
pub struct NotAFrame(pub String);

/// One message from the harness, kept whole.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    value: Json,
}

impl Frame {
    /// Reads a line. A line that is not a JSON object is [`NotAFrame`], with its first characters.
    pub fn parse(line: &str) -> Result<Self, NotAFrame> {
        match serde_json::from_str::<Json>(line) {
            Ok(value) if value.is_object() => Ok(Self { value }),
            _ => Err(NotAFrame(line.chars().take(200).collect())),
        }
    }

    /// A frame made of an object, for the tests and for the diagnostics.
    #[must_use]
    pub fn from_value(value: Json) -> Self {
        Self { value }
    }

    /// The object as the harness wrote it.
    #[must_use]
    pub fn raw(&self) -> &Json {
        &self.value
    }

    /// The object, by value.
    #[must_use]
    pub fn into_raw(self) -> Json {
        self.value
    }

    /// What kind of message it is.
    #[must_use]
    pub fn kind(&self) -> FrameKind {
        match self.text("type") {
            Some("system") => FrameKind::System,
            Some("user") => FrameKind::User,
            Some("assistant") => FrameKind::Assistant,
            Some("stream_event") => FrameKind::StreamEvent,
            Some("result") => FrameKind::Result,
            Some("control_request") => FrameKind::ControlRequest,
            Some("control_response") => FrameKind::ControlResponse,
            Some("control_cancel_request") => FrameKind::ControlCancelRequest,
            Some("keep_alive") => FrameKind::KeepAlive,
            Some(other) => FrameKind::Other(other.to_owned()),
            None => FrameKind::Other(String::new()),
        }
    }

    /// The `subtype` of a system or result message.
    #[must_use]
    pub fn subtype(&self) -> Option<&str> {
        self.text("subtype")
    }

    /// A member of the object.
    #[must_use]
    pub fn get(&self, member: &str) -> Option<&Json> {
        self.value.get(member)
    }

    /// A member that is a text.
    #[must_use]
    pub fn text(&self, member: &str) -> Option<&str> {
        self.value.get(member).and_then(Json::as_str)
    }

    /// The tool use that started the sub-agent this message belongs to; `None` for the main conversation.
    #[must_use]
    pub fn parent_tool_use_id(&self) -> Option<&str> {
        self.text("parent_tool_use_id")
    }

    /// The harness's id of the session the message belongs to.
    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        self.text("session_id")
    }

    /// The message's own id, which the harness gives every message.
    #[must_use]
    pub fn uuid(&self) -> Option<&str> {
        self.text("uuid")
    }

    /// The `message` object of a user or assistant message.
    #[must_use]
    pub fn message(&self) -> Option<&Json> {
        self.value.get("message")
    }

    /// Native echoes may carry an explicit origin or only the synthetic-message flag.
    /// Neither form by itself identifies an owned task or grants authority to act.
    pub(crate) fn has_native_notice_origin(&self) -> bool {
        self.kind() == FrameKind::User
            && match self.get("origin") {
                Some(Json::Object(origin)) => {
                    origin.get("kind").and_then(Json::as_str) == Some("task-notification")
                }
                None | Some(Json::Null) => {
                    self.get("isSynthetic").and_then(Json::as_bool) == Some(true)
                }
                _ => false,
            }
    }

    /// Native task notices can omit run metadata on the user echo. Their task/tool
    /// fields are protocol data only after the harness identifies the frame's origin.
    pub(crate) fn stopped_task_notice(&self) -> Option<(&str, &str)> {
        if !self.has_native_notice_origin() {
            return None;
        }
        let content = self.message()?.get("content")?;
        let text = match content {
            Json::String(text) => text.as_str(),
            Json::Array(blocks) => {
                let mut notices = blocks
                    .iter()
                    .filter(|block| block["type"] == "text")
                    .filter_map(|block| block["text"].as_str())
                    .filter(|text| text.contains("<task-notification>"));
                let first = notices.next()?;
                if notices.next().is_some() {
                    return None;
                }
                first
            }
            _ => return None,
        };
        let packet = one_tag(text, "task-notification")?;
        if !matches!(one_tag(packet, "status"), Some("killed" | "stopped")) {
            return None;
        }
        Some((one_tag(packet, "task-id")?, one_tag(packet, "tool-use-id")?))
    }

    /// The content blocks of the `message`; a text content counts as one text block.
    #[must_use]
    pub fn blocks(&self) -> Vec<Json> {
        match self.message().and_then(|message| message.get("content")) {
            Some(Json::Array(blocks)) => blocks.clone(),
            Some(Json::String(text)) => vec![json!({ "type": "text", "text": text })],
            _ => Vec::new(),
        }
    }
}

fn one_tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    if text.matches(open.as_str()).count() != 1 || text.matches(close.as_str()).count() != 1 {
        return None;
    }
    let (_, content) = text.split_once(open.as_str())?;
    let (value, _) = content.split_once(close.as_str())?;
    (!value.is_empty()).then_some(value)
}

/// A block of text for a user message.
#[must_use]
pub fn text_block(text: &str) -> Json {
    json!({ "type": "text", "text": text })
}

/// A block holding an image, already encoded.
#[must_use]
pub fn image_block(media_type: &str, base64_data: &str) -> Json {
    json!({ "type": "image", "source": { "type": "base64", "media_type": media_type, "data": base64_data } })
}

/// A user message with the given blocks. `uuid` is what the harness echoes back, which is how the adapter
/// knows when it took the message in.
#[must_use]
pub fn user_message(uuid: &str, blocks: Vec<Json>) -> Json {
    json!({
        "type": "user",
        "uuid": uuid,
        "message": { "role": "user", "content": blocks },
    })
}

/// A request to the harness.
#[must_use]
pub fn control_request(request_id: &str, request: Json) -> Json {
    json!({ "type": "control_request", "request_id": request_id, "request": request })
}

/// The answer to a request of the harness.
#[must_use]
pub fn control_success(request_id: &str, response: Json) -> Json {
    json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": response },
    })
}

/// The answer to a request the adapter cannot grant.
#[must_use]
pub fn control_error(request_id: &str, error: &str) -> Json {
    json!({
        "type": "control_response",
        "response": { "subtype": "error", "request_id": request_id, "error": error },
    })
}

/// A value as one line of input: compact, with its line break.
#[must_use]
pub fn encode_line(value: &Json) -> String {
    let mut line = value.to_string();
    line.push('\n');
    line
}

/// How a control response from the harness came out.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlOutcome {
    /// It granted the request, with this body.
    Success(Json),
    /// It refused, with this reason.
    Error(String),
}

impl Frame {
    /// For a control response: the id of the request it answers and how it came out.
    #[must_use]
    pub fn control_outcome(&self) -> Option<(String, ControlOutcome)> {
        let response = self.get("response")?;
        let request_id = response.get("request_id")?.as_str()?.to_owned();
        let outcome = match response.get("subtype").and_then(Json::as_str) {
            Some("success") => {
                ControlOutcome::Success(response.get("response").cloned().unwrap_or(Json::Null))
            }
            _ => ControlOutcome::Error(
                response
                    .get("error")
                    .and_then(Json::as_str)
                    .unwrap_or("the harness refused the request")
                    .to_owned(),
            ),
        };
        Some((request_id, outcome))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_read_whole_and_its_kind_and_members_are_found() {
        let frame = Frame::parse(
            r#"{"type":"assistant","parent_tool_use_id":"toolu_1","session_id":"s","uuid":"u",
                "message":{"content":[{"type":"text","text":"hi"}]},"added_by_a_later_version":true}"#,
        )
        .unwrap();
        assert_eq!(frame.kind(), FrameKind::Assistant);
        assert_eq!(frame.parent_tool_use_id(), Some("toolu_1"));
        assert_eq!(frame.blocks().len(), 1);
        assert!(frame.get("added_by_a_later_version").is_some());
    }

    #[test]
    fn a_kind_nobody_knows_is_a_frame_of_that_name_and_not_an_error() {
        let frame = Frame::parse(r#"{"type":"mystery","x":1}"#).unwrap();
        assert_eq!(frame.kind(), FrameKind::Other("mystery".into()));
    }

    #[test]
    fn a_line_that_is_no_object_is_refused_with_its_beginning() {
        assert!(Frame::parse("warming up...").is_err());
        assert!(Frame::parse("[1,2]").is_err());
        let NotAFrame(shown) = Frame::parse(&"x".repeat(500)).unwrap_err();
        assert_eq!(shown.chars().count(), 200);
    }

    #[test]
    fn text_content_counts_as_one_block_of_text() {
        let frame =
            Frame::parse(r#"{"type":"user","message":{"role":"user","content":"plain"}}"#).unwrap();
        assert_eq!(frame.blocks(), vec![text_block("plain")]);
    }

    #[test]
    fn a_native_stopped_notice_keeps_task_and_tool_ids_without_optional_origin_fields() {
        let packet = "<task-notification>\n<task-id>task:α</task-id>\n<tool-use-id>tool&1</tool-use-id>\n<status>killed</status>\n</task-notification>";
        for content in [json!(packet), json!([{"type":"text","text":packet}])] {
            let frame = Frame::from_value(
                json!({"type":"user","origin":{"kind":"task-notification"},"message":{"role":"user","content":content}}),
            );
            assert_eq!(frame.stopped_task_notice(), Some(("task:α", "tool&1")));
        }
        let synthetic = Frame::from_value(
            json!({"type":"user","isSynthetic":true,"message":{"content":packet}}),
        );
        assert_eq!(synthetic.stopped_task_notice(), Some(("task:α", "tool&1")));
        let conflicting = Frame::from_value(
            json!({"type":"user","isSynthetic":true,"origin":{"kind":"sdk"},"message":{"content":packet}}),
        );
        assert!(conflicting.stopped_task_notice().is_none());
        for content in [
            format!("{packet}{packet}"),
            packet.replace("<status>killed</status>", "<status>failed</status>"),
            packet.replace(
                "<task-id>task:α</task-id>",
                "<task-id>task:α</task-id><task-id>other</task-id>",
            ),
            packet.replace("<tool-use-id>tool&1</tool-use-id>", ""),
        ] {
            let frame = Frame::from_value(
                json!({"type":"user","origin":{"kind":"task-notification"},"message":{"content":content}}),
            );
            assert!(frame.stopped_task_notice().is_none());
        }
        for origin in [json!(null), json!({"kind":"sdk"})] {
            let frame = Frame::from_value(
                json!({"type":"user","origin":origin,"message":{"content":packet}}),
            );
            assert!(
                frame.stopped_task_notice().is_none(),
                "text alone is never native provenance"
            );
        }
        let assistant = Frame::from_value(
            json!({"type":"assistant","origin":{"kind":"task-notification"},"message":{"content":packet}}),
        );
        assert!(assistant.stopped_task_notice().is_none());
    }

    #[test]
    fn a_control_response_says_which_request_it_answers_and_how() {
        let ok = Frame::parse(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"r1","response":{"mode":"default"}}}"#,
        )
        .unwrap();
        assert_eq!(
            ok.control_outcome(),
            Some((
                "r1".into(),
                ControlOutcome::Success(json!({ "mode": "default" }))
            ))
        );
        let failed = Frame::parse(
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"r2","error":"no"}}"#,
        )
        .unwrap();
        assert_eq!(
            failed.control_outcome(),
            Some(("r2".into(), ControlOutcome::Error("no".into())))
        );
    }

    #[test]
    fn what_the_adapter_writes_is_one_compact_line() {
        let line = encode_line(&user_message("u-1", vec![text_block("hello")]));
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
        let written: Json = serde_json::from_str(&line).unwrap();
        assert_eq!(written["uuid"], "u-1");
        assert_eq!(written["message"]["content"][0]["text"], "hello");
    }
}

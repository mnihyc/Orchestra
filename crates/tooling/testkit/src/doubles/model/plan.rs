//! What the two renderers share: the frames a body is played from, the figures of use, and the context of one response.
//!
//! A response is rendered whole, ahead of sending, into frames: bytes to send, a wait, or the end of the
//! connection. Playing the frames is the server's work; rendering is a plain function of the script and the
//! request, which is what makes the same script give the same bytes.

use std::time::Duration;

use bytes::Bytes;
use serde_json::{Map, Value};

use super::script::{CHARS_PER_TOKEN, Segment, Step};

/// One thing to do while a body is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// Bytes to send.
    Bytes(Bytes),
    /// Wait before the next frame.
    Wait(Duration),
    /// Abort the connection: nothing more is sent, and the client sees the body break off.
    Abort,
}

/// What a renderer needs to know of the request and the server.
#[derive(Debug, Clone)]
pub struct Ctx {
    /// The number of the request, counted from 1; it numbers the ids of the response.
    pub seq: u64,
    /// The model to report.
    pub model: String,
    /// The moment to report, in seconds since the epoch.
    pub created_at: i64,
    /// Whether the request asked for the encrypted form of the reasoning.
    pub encrypted_reasoning: bool,
    /// The response the request continues.
    pub previous_response_id: Option<String>,
}

/// The figures of use a response reports, counted as the model contract counts them: the cached and the
/// written tokens are parts of the input, the reasoning is part of the output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    /// Tokens of input.
    pub input: i64,
    /// Tokens of output.
    pub output: i64,
    /// Of the input, those read from a cache.
    pub cached: i64,
    /// Of the output, the reasoning.
    pub reasoning: i64,
    /// Of the input, those written to a cache.
    pub cache_write: i64,
    /// What the upstream states it charged, as a decimal.
    pub cost: Option<String>,
}

/// The tokens a text of `chars` characters is taken to hold: at least one for any text.
#[must_use]
pub fn estimate(chars: usize) -> i64 {
    i64::try_from(chars.div_ceil(CHARS_PER_TOKEN)).unwrap_or(i64::MAX)
}

/// The figures a segment reports: those the script states, and for the rest an estimate from the text.
#[must_use]
pub fn usage_of(segment: &Segment, input_chars: usize) -> Usage {
    let (mut said, mut thought) = (0_usize, 0_usize);
    for step in &segment.steps {
        match step {
            Step::Text { text, .. } => said += text.chars().count(),
            Step::Reasoning { text, .. } => thought += text.chars().count(),
            Step::Tool { arguments, .. } => said += arguments.chars().count(),
            Step::Custom { input, .. } => said += input.chars().count(),
            _ => {}
        }
    }
    let stated = &segment.usage;
    let input = stated.input.unwrap_or_else(|| estimate(input_chars).max(1));
    let output = stated
        .output
        .unwrap_or_else(|| estimate(said) + estimate(thought));
    let cached = stated.cached.unwrap_or(0).clamp(0, input);
    Usage {
        input,
        output,
        cached,
        reasoning: stated
            .reasoning
            .unwrap_or_else(|| estimate(thought))
            .clamp(0, output),
        cache_write: stated.cache_write.unwrap_or(0).clamp(0, input - cached),
        cost: stated.cost.clone(),
    }
}

/// A body being written: server-sent events and comments, waits, and the end.
#[derive(Debug, Default)]
pub struct Wire {
    frames: Vec<Frame>,
    sequence: u64,
    bare: bool,
}

impl Wire {
    /// An empty body.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty body whose events have no `event:` line, only data, as a gateway writes them.
    #[must_use]
    pub fn bare() -> Self {
        Self {
            bare: true,
            ..Self::default()
        }
    }

    /// The number the next event of the Responses format's stream carries.
    pub fn next_sequence(&mut self) -> u64 {
        let next = self.sequence;
        self.sequence += 1;
        next
    }

    /// Sends one event: its name and its data.
    pub fn event(&mut self, name: &str, data: &Value) {
        let text = if self.bare {
            format!("data: {data}\n\n")
        } else {
            format!("event: {name}\ndata: {data}\n\n")
        };
        self.frames.push(Frame::Bytes(Bytes::from(text)));
    }

    /// Sends an event whose data is written as it stands, which need not be JSON, under a name or, with
    /// none, as a line of data alone.
    pub fn raw(&mut self, name: Option<&str>, data: &str) {
        let text = match name {
            Some(name) if !self.bare => format!("event: {name}\ndata: {data}\n\n"),
            _ => format!("data: {data}\n\n"),
        };
        self.frames.push(Frame::Bytes(Bytes::from(text)));
    }

    /// Sends the marker a gateway ends a stream with: `data: [DONE]`, under the event name `data` when the
    /// events have names.
    pub fn done_marker(&mut self) {
        let text = if self.bare {
            "data: [DONE]\n\n"
        } else {
            "event: data\ndata: [DONE]\n\n"
        };
        self.frames
            .push(Frame::Bytes(Bytes::from_static(text.as_bytes())));
    }

    /// Sends one event of the Responses format, whose data begins with its type and its sequence number,
    /// and goes on with the members of `rest`.
    pub fn numbered(&mut self, name: &str, rest: Value) {
        let mut data = Map::new();
        data.insert("type".to_owned(), Value::from(name));
        data.insert(
            "sequence_number".to_owned(),
            Value::from(self.next_sequence()),
        );
        if let Value::Object(members) = rest {
            data.extend(members);
        }
        self.event(name, &Value::Object(data));
    }

    /// Sends a comment line, which carries nothing and keeps a quiet stream alive.
    pub fn comment(&mut self, text: &str) {
        self.frames
            .push(Frame::Bytes(Bytes::from(format!(": {text}\n\n"))));
    }

    /// Waits before what follows, if there is anything to wait.
    pub fn wait(&mut self, duration: Duration) {
        if !duration.is_zero() {
            self.frames.push(Frame::Wait(duration));
        }
    }

    /// Aborts the connection here.
    pub fn abort(&mut self) {
        self.frames.push(Frame::Abort);
    }

    /// The frames written.
    #[must_use]
    pub fn finish(self) -> Vec<Frame> {
        self.frames
    }
}

/// What a stream does at its end, when a step ended it early.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// All of it was sent.
    Completed,
    /// An error event was sent, and nothing after it.
    StreamError,
    /// The connection was aborted.
    Cut,
    /// The body was ended early.
    Truncated,
}

/// The concatenation of the bytes of the frames, for tests.
#[cfg(test)]
#[must_use]
pub fn body_of(frames: &[Frame]) -> Vec<u8> {
    let mut body = Vec::new();
    for frame in frames {
        if let Frame::Bytes(bytes) = frame {
            body.extend_from_slice(bytes);
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doubles::model::script::Script;

    fn segment(text: &str) -> Segment {
        Script::parse(text).unwrap().segment(0)
    }

    #[test]
    fn figures_that_are_not_stated_are_estimated_from_the_text() {
        let usage = usage_of(&segment("say 0123456789\nthink abcd"), 40);
        assert_eq!(usage.input, 10);
        assert_eq!(
            usage.output,
            3 + 1,
            "ten characters and four, a token to four characters"
        );
        assert_eq!(usage.reasoning, 1);
        assert_eq!((usage.cached, usage.cache_write), (0, 0));
        assert_eq!(
            usage_of(&segment(""), 0).input,
            1,
            "a request is at least one token"
        );
        assert_eq!(usage_of(&segment(""), 0).output, 0);
    }

    #[test]
    fn figures_that_are_stated_win_and_no_part_is_larger_than_its_whole() {
        let usage = usage_of(
            &segment("say hi\nusage 100 20 cached=500 reasoning=90 cache_write=70 cost=0.5"),
            0,
        );
        assert_eq!((usage.input, usage.output), (100, 20));
        assert_eq!(usage.cached, 100, "held to the input");
        assert_eq!(usage.reasoning, 20, "held to the output");
        assert_eq!(usage.cache_write, 0, "what is cached is not also written");
        assert_eq!(usage.cost.as_deref(), Some("0.5"));
    }

    #[test]
    fn events_are_written_the_way_the_wire_has_them() {
        let mut wire = Wire::new();
        wire.numbered(
            "response.created",
            serde_json::json!({ "response": { "id": "r" } }),
        );
        wire.comment("keep-alive");
        wire.event("ping", &serde_json::json!({ "type": "ping" }));
        wire.wait(Duration::ZERO);
        wire.wait(Duration::from_millis(5));
        wire.abort();
        let frames = wire.finish();
        assert_eq!(
            String::from_utf8(body_of(&frames)).unwrap(),
            "event: response.created\ndata: {\"type\":\"response.created\",\"sequence_number\":0,\"response\":{\"id\":\"r\"}}\n\n\
             : keep-alive\n\n\
             event: ping\ndata: {\"type\":\"ping\"}\n\n"
        );
        assert_eq!(frames.len(), 5, "a wait of nothing is no wait");
        assert_eq!(frames[3], Frame::Wait(Duration::from_millis(5)));
        assert_eq!(frames[4], Frame::Abort);
    }
}

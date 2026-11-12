//! Streaming usage observation.
//!
//! The forwarder is a plain relay, and counts what passes through it. A [`UsageObserver`] reads the response
//! as it streams by, server-sent events and plain JSON alike, and reports what the upstream says was used,
//! best-effort: input, output, cached and reasoning tokens, and a cost where the upstream states one. It
//! never buffers the stream and never delays it, and it can be asked what it has seen at any moment, so
//! that a stream that is cut still reports what came before the cut.
//!
//! Every count follows one convention: cached input tokens and tokens written to a cache are parts of the
//! input, reasoning tokens are part of the output, and the total is input plus output. An observer of an
//! upstream that reports them apart adds them in ([`TokenUsage::from_parts`]).
//!
//! [`SseParser`] cuts a stream of bytes into server-sent events however the bytes were chunked, which is
//! what an observer of an event stream needs first.

use orch_agent_contract::events::TokenUsage;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What the upstream says a response used.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ObservedUsage {
    /// Successful native search operations, only where the vendor reports them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_searches: Option<i64>,
    /// Successful native page fetches; absence never proves zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_fetches: Option<i64>,
    /// Other declared native quantities; missing is unsupported and a present null is unreported.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub declared_counters: std::collections::BTreeMap<String, Option<i64>>,
    /// The processing tier or speed the upstream actually reported, never the requested choice.
    pub service_tier: Option<String>,
    /// The independently reported capacity class, where this interface supports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_tier: Option<String>,
    /// One-hour writes, a subset of cache_write_tokens, where independently known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h_tokens: Option<i64>,
    /// Twelve-hour writes inside the inclusive cache-write count; absent is not measured zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_12h_tokens: Option<i64>,
    /// The model that answered, as the response names it.
    pub model: Option<String>,
    /// What was used, in tokens.
    pub tokens: TokenUsage,
    /// What the upstream states it charged, as a decimal string in its own unit, where it states one.
    pub cost: Option<String>,
    /// The extension-declared original cost unit, only when the corresponding actual cost was read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_unit: Option<String>,
    /// The upstream's own id for the response.
    pub request_ref: Option<String>,
}

/// Facts observed independently of whether the response contained a usage object.
#[derive(Debug, Clone, Default)]
pub struct ResponseFacts {
    /// The upstream response identity, even before its final token report.
    pub request_id: Option<String>,
    /// A real output delta was observed, rather than a keepalive or opening event.
    pub output_started: bool,
    /// The protocol supplied its terminal event or a complete JSON envelope.
    pub completed: bool,
    /// A structured upstream error or refusal, with its original words.
    pub failure: Option<serde_json::Value>,
}

/// Native endpoint declarations for this exact request, not manager enablement or an authority proof.
pub type NativeBindings =
    std::collections::BTreeMap<String, Option<orch_provider_contract::capabilities::Binding>>;

/// Reads one response as it streams by.
pub trait UsageObserver: Send {
    /// Header status belongs to this same exchange, independent of the reply's JSON vocabulary.
    fn response_status(&mut self, _status: u16) {}

    /// A successful whole-body side result is not proved by an otherwise valid prefix of a cut body.
    fn finish(&mut self, _whole: bool) {}

    /// Counter-only side replies make no claim about generation tokens.
    fn tokens_reported(&self) -> bool {
        true
    }

    /// Configure actual endpoint/model counters on the SAME byte parser. Unsupported readers keep tokens
    /// and make the additional quantities unknown rather than borrowing incompatible native counts.
    fn configure_capabilities(&mut self, bindings: &NativeBindings) -> bool {
        bindings
            .values()
            .flatten()
            .all(|binding| binding.counters.is_empty())
    }
    /// Feeds the next piece of the response body, as it passes. It must not buffer the body or hold it up.
    fn observe(&mut self, chunk: &[u8]);

    /// What has been seen so far, or `None` when the response said nothing about use. Best-effort: it may
    /// be asked at any time, also after the stream was cut.
    fn usage(&self) -> Option<ObservedUsage>;

    /// Optional facts from the same parser, without a second body buffer or usage requirement.
    fn response_facts(&self) -> ResponseFacts {
        ResponseFacts::default()
    }
}

/// One event of a server-sent event stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` field, when there was one.
    pub event: Option<String>,
    /// The `data:` lines, joined with a newline.
    pub data: String,
    /// The `id:` field, when there was one.
    pub id: Option<String>,
}

/// Cuts a stream of bytes into server-sent events, however it was chunked: lines end with `\n`, `\r\n` or
/// `\r`; a line that begins with `:` is a comment; a blank line ends an event; one space after the colon of
/// a field is dropped. An event with no data is not reported.
#[derive(Debug, Default)]
pub struct SseParser {
    buffer: Vec<u8>,
    current: SseEvent,
    has_data: bool,
    /// A `\r` ended the last line and a `\n` right after it belongs to it.
    after_cr: bool,
}

impl SseParser {
    /// A parser that has seen nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds the next bytes and returns the events they completed.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let mut events = Vec::new();
        for &byte in chunk {
            if self.after_cr {
                self.after_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\n' => self.end_line(&mut events),
                b'\r' => {
                    self.after_cr = true;
                    self.end_line(&mut events);
                }
                other => self.buffer.push(other),
            }
        }
        events
    }

    fn end_line(&mut self, events: &mut Vec<SseEvent>) {
        let line = String::from_utf8_lossy(&self.buffer).into_owned();
        self.buffer.clear();
        if line.is_empty() {
            if self.has_data {
                events.push(std::mem::take(&mut self.current));
            } else {
                self.current = SseEvent::default();
            }
            self.has_data = false;
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = line.split_once(':').unwrap_or((line.as_str(), ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.current.event = Some(value.to_owned()),
            "data" => {
                if self.has_data {
                    self.current.data.push('\n');
                }
                self.current.data.push_str(value);
                self.has_data = true;
            }
            "id" => self.current.id = Some(value.to_owned()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(chunks: &[&[u8]]) -> Vec<SseEvent> {
        let mut parser = SseParser::new();
        chunks.iter().flat_map(|chunk| parser.push(chunk)).collect()
    }

    #[test]
    fn events_come_out_however_the_stream_was_chunked() {
        let stream = b"event: message_start\ndata: {\"a\":1}\n\nevent: delta\ndata: x\ndata: y\n\n: keep-alive\n\ndata: last\n\n";
        let whole = all(&[stream]);
        assert_eq!(whole.len(), 3);
        assert_eq!(whole[0].event.as_deref(), Some("message_start"));
        assert_eq!(whole[0].data, "{\"a\":1}");
        assert_eq!(
            whole[1].data, "x\ny",
            "several data lines are one event, joined by a newline"
        );
        assert_eq!(whole[2].event, None);
        let bytes: Vec<&[u8]> = stream.chunks(1).collect();
        assert_eq!(
            all(&bytes),
            whole,
            "one byte at a time gives the same events"
        );
        let halves = [&stream[..17], &stream[17..40], &stream[40..]];
        assert_eq!(all(&halves), whole);
    }

    #[test]
    fn all_three_line_endings_end_a_line_and_a_comment_or_an_empty_event_is_not_an_event() {
        let events =
            all(&[b"data: a\r\n\r\ndata: b\r\rdata: c\n\n: only a comment\n\nevent: nothing\n\n"]);
        let data: Vec<_> = events.iter().map(|e| e.data.as_str()).collect();
        assert_eq!(data, ["a", "b", "c"]);
    }

    #[test]
    fn a_crlf_split_between_two_chunks_is_one_line_ending() {
        let events = all(&[b"data: a\r", b"\n\r", b"\ndata: b\n\n"]);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "a");
        assert_eq!(events[1].data, "b");
    }

    #[test]
    fn a_multibyte_character_split_between_chunks_survives() {
        let text = "data: größe\n\n".as_bytes();
        let events = all(&[&text[..9], &text[9..]]);
        assert_eq!(events[0].data, "größe");
    }

    #[test]
    fn the_id_and_a_field_without_a_colon_are_read() {
        let events = all(&[b"id: 7\ndata\n\nid:8\ndata:z\n\n"]);
        assert_eq!(events[0].id.as_deref(), Some("7"));
        assert_eq!(events[0].data, "");
        assert_eq!(events[1].data, "z");
    }

    #[test]
    fn usage_that_is_observed_reads_and_writes_as_a_figure() {
        let seen = ObservedUsage {
            capacity_tier: None,
            cache_write_1h_tokens: None,
            cache_write_12h_tokens: None,
            web_searches: None,
            web_fetches: None,
            declared_counters: Default::default(),
            cost_unit: None,
            service_tier: None,
            model: Some("gpt-6-sol".into()),
            tokens: TokenUsage::from_parts(orch_agent_contract::events::TokensReportedApart {
                input: 10,
                cache_read: 90,
                cache_write: 0,
                output: 15,
                reasoning: 5,
            }),
            cost: Some("0.0021".into()),
            request_ref: Some("resp_1".into()),
        };
        assert_eq!(seen.tokens.total_tokens, 120);
        let back: ObservedUsage =
            serde_json::from_value(serde_json::to_value(&seen).unwrap()).unwrap();
        assert_eq!(back, seen);
    }
}

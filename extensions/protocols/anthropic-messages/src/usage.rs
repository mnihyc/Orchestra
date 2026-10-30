//! Streaming usage observation.
//!
//! [`MessagesUsage`] reads what the vendor says a message used, as the answer passes by. The Messages format
//! answers in one of two ways and the observer tells them apart by the first byte it sees: a stream of
//! server-sent events (`event:` and `data:` lines, one `data:` document for each event) or one JSON document.
//!
//! In a stream, figures come twice. `message_start` carries the message with a `usage` that says what the
//! prompt took (`input_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`) and an `output_tokens`
//! that is only a start; `message_delta` carries a `usage` whose members are *cumulative*: each one that is
//! present replaces what was known, and the final `output_tokens` is the count of the whole answer. A stream
//! that is cut after `message_start` therefore still reports the prompt, with the output counted so far at the
//! vendor's own start figure; one that is cut before it reports nothing. A plain answer carries `usage` at the
//! top of the message.
//!
//! The vendor counts the parts apart: `input_tokens` leaves out what was read from or written to a cache, and
//! `output_tokens` includes the reasoning, of which `output_tokens_details.thinking_tokens` says how much. The
//! counts of the model contract put cached and written tokens inside the input and reasoning inside the output,
//! so the parts are added into them and the total is input plus output. The answer to a request to count
//! tokens is `{"input_tokens": n}` with no `usage`: that is a question answered, not use, and it reads as
//! nothing.
//!
//! A gateway may add `usage.cost`, what it charged; it is kept as the text the gateway wrote, in plain decimal
//! form.
//!
//! The observer is a tap, not a stage: it copies what it needs out of each piece and never holds the piece
//! back. A plain answer has to be whole before it can be read, so up to [`MAX_PLAIN_BODY_BYTES`] of it are
//! kept; past that the answer is not read. A body that is compressed cannot be read at all and yields
//! nothing: the forwarder asks for an uncompressed answer or decodes it first.

use orch_agent_contract::events::{TokenUsage, TokensReportedApart};
use orch_model_contract::usage::{ObservedUsage, SseEvent, SseParser, UsageObserver};
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

/// The most of a plain JSON answer that is kept to be read at its end. A model's longest answer is a few
/// hundred thousand tokens, about a megabyte of text; eight leaves room for long structured content.
pub const MAX_PLAIN_BODY_BYTES: usize = 8 * 1024 * 1024;

/// The event that opens a stream and carries the message with the prompt's figures.
const START_EVENT: &str = "message_start";

/// The event that carries the cumulative figures of the answer.
const DELTA_EVENT: &str = "message_delta";

/// A message object, as the answer to a plain request and as `message_start` carries it. Every other
/// member, the content above all, is skipped without being kept.
#[derive(Deserialize)]
struct MessageView {
    id: Option<Value>,
    model: Option<Value>,
    usage: Option<Box<RawValue>>,
}

/// The `message_start` event.
#[derive(Deserialize)]
struct StartView {
    message: Option<MessageView>,
}

/// The `message_delta` event.
#[derive(Deserialize)]
struct DeltaView {
    usage: Option<Box<RawValue>>,
}

/// The `type` of an event whose name came in no `event:` line.
#[derive(Deserialize)]
struct Probe<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<&'a str>,
}

/// The members of a usage object that matter here.
#[derive(Deserialize)]
struct UsageView {
    input_tokens: Option<Value>,
    output_tokens: Option<Value>,
    cache_creation_input_tokens: Option<Value>,
    cache_creation: Option<Value>,
    cache_read_input_tokens: Option<Value>,
    output_tokens_details: Option<Value>,
    cost: Option<Box<RawValue>>,
}

/// A count as a gateway may write it: a whole number, a number with a fraction, or a number in a string.
/// Anything else, and anything negative, is no count.
fn count(value: Option<&Value>) -> Option<i64> {
    let number = match value? {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_u64().and_then(|n| i64::try_from(n).ok()))
            .or_else(|| {
                number
                    .as_f64()
                    .filter(|fraction| fraction.is_finite())
                    .map(|fraction| fraction.round() as i64)
            })?,
        Value::String(text) => text.trim().parse::<i64>().ok()?,
        _ => return None,
    };
    (number >= 0).then_some(number)
}

/// The text of a string value.
fn text(value: Option<Value>) -> Option<String> {
    match value? {
        Value::String(text) if !text.is_empty() => Some(text),
        _ => None,
    }
}

/// The tokens written to a cache: the total the vendor states, or else the sum of its two breakdowns.
fn written_to_cache(usage: &UsageView) -> Option<i64> {
    count(usage.cache_creation_input_tokens.as_ref()).or_else(|| {
        let breakdown = usage.cache_creation.as_ref()?;
        let short = count(breakdown.get("ephemeral_5m_input_tokens"));
        let long = count(breakdown.get("ephemeral_1h_input_tokens"));
        (short.is_some() || long.is_some()).then(|| short.unwrap_or(0) + long.unwrap_or(0))
    })
}

/// Replaces what is known of a count by a new word on it, if there is one.
fn replace(slot: &mut Option<i64>, new: Option<i64>) {
    if new.is_some() {
        *slot = new;
    }
}

/// What the vendor has said of the counts so far; each member is the latest word on it.
#[derive(Debug, Clone, Default)]
struct Figures {
    input: Option<i64>,
    cache_read: Option<i64>,
    cache_write: Option<i64>,
    output: Option<i64>,
    thinking: Option<i64>,
    cost: Option<String>,
}

impl Figures {
    /// Takes in a usage object: every member it carries replaces what was known of that member.
    fn take(&mut self, usage: &UsageView) {
        replace(&mut self.input, count(usage.input_tokens.as_ref()));
        replace(
            &mut self.cache_read,
            count(usage.cache_read_input_tokens.as_ref()),
        );
        replace(&mut self.cache_write, written_to_cache(usage));
        replace(&mut self.output, count(usage.output_tokens.as_ref()));
        replace(
            &mut self.thinking,
            count(
                usage
                    .output_tokens_details
                    .as_ref()
                    .and_then(|details| details.get("thinking_tokens")),
            ),
        );
        if let Some(cost) = usage
            .cost
            .as_ref()
            .and_then(|cost| crate::decimal::plain(cost.get()))
        {
            self.cost = Some(cost);
        }
    }

    /// The counts as the contract counts them, if the vendor said anything of the input or the output.
    fn tokens(&self) -> Option<TokenUsage> {
        if self.input.is_none() && self.output.is_none() {
            return None;
        }
        let output = self.output.unwrap_or(0);
        let thinking = self.thinking.unwrap_or(0).min(output);
        Some(TokenUsage::from_parts(TokensReportedApart {
            input: self.input.unwrap_or(0),
            cache_read: self.cache_read.unwrap_or(0),
            cache_write: self.cache_write.unwrap_or(0),
            output: output - thinking,
            reasoning: thinking,
        }))
    }
}

/// What has been learned of the message so far.
#[derive(Debug, Clone, Default)]
struct Seen {
    id: Option<String>,
    model: Option<String>,
    figures: Figures,
}

impl Seen {
    /// Takes in a usage object given as raw text.
    fn take_usage(&mut self, raw: &RawValue) {
        if let Ok(usage) = serde_json::from_str::<UsageView>(raw.get()) {
            self.figures.take(&usage);
        }
    }

    /// Takes in a message object.
    fn take_message(&mut self, message: MessageView) {
        if let Some(id) = text(message.id) {
            self.id = Some(id);
        }
        if let Some(model) = text(message.model) {
            self.model = Some(model);
        }
        if let Some(raw) = message.usage {
            self.take_usage(&raw);
        }
    }

    /// Takes in one event of a stream.
    fn take_event(&mut self, event: &SseEvent) {
        let kind = match event.event.as_deref() {
            Some(name) => name,
            None => match serde_json::from_str::<Probe<'_>>(&event.data)
                .ok()
                .and_then(|probe| probe.kind)
            {
                Some(kind) => kind,
                None => return,
            },
        };
        match kind {
            START_EVENT => {
                if let Ok(StartView {
                    message: Some(message),
                }) = serde_json::from_str::<StartView>(&event.data)
                {
                    self.take_message(message);
                }
            }
            DELTA_EVENT => {
                if let Ok(DeltaView { usage: Some(raw) }) =
                    serde_json::from_str::<DeltaView>(&event.data)
                {
                    self.take_usage(&raw);
                }
            }
            _ => {}
        }
    }

    /// What was seen, if it holds figures.
    fn observed(&self) -> Option<ObservedUsage> {
        Some(ObservedUsage {
            model: self.model.clone(),
            tokens: self.figures.tokens()?,
            cost: self.figures.cost.clone(),
            request_ref: self.id.clone(),
        })
    }
}

/// What kind of body the observer is reading.
#[derive(Debug, Default)]
enum Body {
    /// Nothing but white space has come yet.
    #[default]
    Undecided,
    /// One JSON document, kept until it can be read; `None` once it outgrew [`MAX_PLAIN_BODY_BYTES`].
    Plain(Option<Vec<u8>>),
    /// Server-sent events.
    Stream(SseParser),
}

/// Reads the usage of one answer of the Messages format as it passes by.
#[derive(Debug, Default)]
pub struct MessagesUsage {
    body: Body,
    seen: Seen,
}

impl MessagesUsage {
    /// An observer that has seen nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl UsageObserver for MessagesUsage {
    fn observe(&mut self, chunk: &[u8]) {
        let Self { body, seen } = self;
        if matches!(body, Body::Undecided) {
            match chunk.iter().find(|byte| !byte.is_ascii_whitespace()) {
                None => return,
                Some(b'{') => *body = Body::Plain(Some(Vec::new())),
                Some(_) => *body = Body::Stream(SseParser::new()),
            }
        }
        match body {
            Body::Undecided => {}
            Body::Plain(kept) => {
                if let Some(bytes) = kept {
                    if bytes.len() + chunk.len() <= MAX_PLAIN_BODY_BYTES {
                        bytes.extend_from_slice(chunk);
                    } else {
                        *kept = None;
                    }
                }
            }
            Body::Stream(parser) => {
                for event in parser.push(chunk) {
                    seen.take_event(&event);
                }
            }
        }
    }

    fn usage(&self) -> Option<ObservedUsage> {
        if let Body::Plain(Some(bytes)) = &self.body {
            let mut seen = self.seen.clone();
            if let Ok(message) = serde_json::from_slice::<MessageView>(bytes) {
                seen.take_message(message);
            }
            return seen.observed();
        }
        self.seen.observed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream as the vendor writes one: a text answer.
    const STREAM: &str = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-opus-5\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":25,\"output_tokens\":1}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: ping\n",
        "data: {\"type\": \"ping\"}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":15}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    fn feed(chunks: &[&[u8]]) -> MessagesUsage {
        let mut observer = MessagesUsage::new();
        for chunk in chunks {
            observer.observe(chunk);
        }
        observer
    }

    fn whole(body: &str) -> Option<ObservedUsage> {
        feed(&[body.as_bytes()]).usage()
    }

    #[test]
    fn a_stream_reports_the_prompt_from_its_start_and_the_answer_from_its_last_delta() {
        let seen = whole(STREAM).unwrap();
        assert_eq!(seen.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(seen.request_ref.as_deref(), Some("msg_1"));
        assert_eq!(seen.tokens.input_tokens, 25);
        assert_eq!(
            seen.tokens.output_tokens, 15,
            "the cumulative figure replaces the start"
        );
        assert_eq!(seen.tokens.total_tokens, 40);
        assert_eq!(seen.tokens.cached_input_tokens, 0);
        assert_eq!(seen.cost, None);
    }

    #[test]
    fn however_the_stream_is_chunked_the_figures_are_the_same() {
        let expected = whole(STREAM);
        for size in [1, 2, 3, 7, 64, 500] {
            let chunks: Vec<&[u8]> = STREAM.as_bytes().chunks(size).collect();
            assert_eq!(feed(&chunks).usage(), expected, "chunks of {size}");
        }
    }

    #[test]
    fn a_stream_cut_after_its_start_still_reports_the_prompt_and_what_the_vendor_had_counted() {
        let cut = &STREAM[..STREAM.find("event: message_delta").unwrap()];
        let seen = whole(cut).unwrap();
        assert_eq!(seen.tokens.input_tokens, 25);
        assert_eq!(seen.tokens.output_tokens, 1);
        assert_eq!(seen.model.as_deref(), Some("claude-opus-5"));
    }

    #[test]
    fn a_stream_cut_before_its_start_or_inside_it_says_nothing() {
        let first = STREAM.find("event: content_block_start").unwrap();
        assert!(whole(&STREAM[..first]).is_some());
        assert_eq!(
            whole(&STREAM[..first - 30]),
            None,
            "a start cut in two is no start"
        );
        assert_eq!(whole(""), None);
    }

    #[test]
    fn the_cache_is_part_of_the_input_and_the_figures_of_a_delta_replace_those_of_the_start() {
        // The vendor's own example of an answer that used a server tool: the delta repeats the input figures.
        let stream = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_2\",\"model\":\"claude-opus-5\",\"usage\":{\"input_tokens\":2679,\"cache_creation_input_tokens\":100,\"cache_read_input_tokens\":300,\"output_tokens\":3}}}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":10682,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":500,\"output_tokens\":510,\"server_tool_use\":{\"web_search_requests\":1}}}\n\n",
        );
        let seen = whole(stream).unwrap();
        assert_eq!(
            seen.tokens.input_tokens,
            10682 + 500,
            "input and cache read together"
        );
        assert_eq!(seen.tokens.cached_input_tokens, 500);
        assert_eq!(seen.tokens.output_tokens, 510);
        assert_eq!(seen.tokens.total_tokens, 10682 + 500 + 510);
    }

    #[test]
    fn a_delta_that_says_nothing_of_a_member_leaves_what_the_start_said() {
        let stream = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":40,\"cache_read_input_tokens\":10,\"cache_creation_input_tokens\":5,\"output_tokens\":1}}}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"input_tokens\":null,\"output_tokens\":9}}\n\n",
        );
        let seen = whole(stream).unwrap();
        assert_eq!(seen.tokens.input_tokens, 40 + 10 + 5);
        assert_eq!(seen.tokens.cached_input_tokens, 10);
        assert_eq!(seen.tokens.output_tokens, 9);
    }

    #[test]
    fn the_thinking_tokens_are_the_reasoning_part_of_the_output() {
        let seen = whole(r#"{"id":"m","type":"message","model":"x","usage":{"input_tokens":5,"output_tokens":100,"output_tokens_details":{"thinking_tokens":40}}}"#).unwrap();
        assert_eq!(
            seen.tokens.output_tokens, 100,
            "the vendor's output already holds them"
        );
        assert_eq!(seen.tokens.reasoning_tokens, 40);
        assert_eq!(seen.tokens.total_tokens, 105);
        // More thinking than output cannot be: it is held to the output.
        let seen = whole(r#"{"usage":{"input_tokens":5,"output_tokens":10,"output_tokens_details":{"thinking_tokens":99}}}"#).unwrap();
        assert_eq!(seen.tokens.reasoning_tokens, 10);
        assert!(seen.tokens.is_consistent());
    }

    #[test]
    fn a_plain_message_is_read_from_its_usage_member_however_it_arrives() {
        let body = r#"{"id":"msg_3","type":"message","role":"assistant","model":"claude-haiku-5.5","content":[{"type":"text","text":"{\"usage\": 1}"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":2095,"cache_creation_input_tokens":2051,"cache_read_input_tokens":2051,"output_tokens":503,"service_tier":"standard","cache_creation":{"ephemeral_1h_input_tokens":0,"ephemeral_5m_input_tokens":2051}}}"#;
        let expected = whole(body).unwrap();
        assert_eq!(expected.tokens.input_tokens, 2095 + 2051 + 2051);
        assert_eq!(expected.tokens.cached_input_tokens, 2051);
        assert_eq!(expected.tokens.output_tokens, 503);
        assert_eq!(expected.request_ref.as_deref(), Some("msg_3"));
        assert_eq!(expected.model.as_deref(), Some("claude-haiku-5.5"));
        for size in [1, 9, 100] {
            let chunks: Vec<&[u8]> = body.as_bytes().chunks(size).collect();
            assert_eq!(
                feed(&chunks).usage(),
                Some(expected.clone()),
                "chunks of {size}"
            );
        }
    }

    #[test]
    fn tokens_written_to_a_cache_are_summed_from_the_breakdown_when_no_total_is_given() {
        let seen = whole(r#"{"usage":{"input_tokens":10,"output_tokens":1,"cache_creation":{"ephemeral_5m_input_tokens":7,"ephemeral_1h_input_tokens":3}}}"#).unwrap();
        assert_eq!(seen.tokens.input_tokens, 20);
    }

    #[test]
    fn a_count_of_tokens_is_a_question_answered_and_not_use() {
        assert_eq!(whole(r#"{"input_tokens":2095}"#), None);
    }

    #[test]
    fn an_error_a_stream_error_or_a_cut_document_says_nothing() {
        assert_eq!(
            whole(r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow"}}"#),
            None
        );
        let failing = concat!(
            "event: error\n",
            "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n",
        );
        assert_eq!(whole(failing), None);
        assert_eq!(whole(r#"{"id":"m","usage":{"input_tokens":5,"outp"#), None);
    }

    #[test]
    fn a_stream_whose_start_has_no_usage_at_all_has_no_figures() {
        // The vendor's own example of a thinking stream: neither the start nor the delta carries usage.
        let stream = concat!(
            "event: message_start\n",
            "data: {\"type\": \"message_start\", \"message\": {\"id\": \"msg_01\", \"type\": \"message\", \"role\": \"assistant\", \"content\": [], \"model\": \"claude-opus-5-5\", \"stop_reason\": null, \"stop_sequence\": null}}\n\n",
            "event: message_delta\n",
            "data: {\"type\": \"message_delta\", \"delta\": {\"stop_reason\": \"end_turn\", \"stop_sequence\": null}}\n\n",
            "event: message_stop\n",
            "data: {\"type\": \"message_stop\"}\n\n",
        );
        assert_eq!(whole(stream), None);
    }

    #[test]
    fn the_cost_a_gateway_states_is_kept_as_the_decimal_it_wrote() {
        let seen =
            whole(r#"{"usage":{"input_tokens":1,"output_tokens":1,"cost":0.0000123}}"#).unwrap();
        assert_eq!(seen.cost.as_deref(), Some("0.0000123"));
        let seen =
            whole(r#"{"usage":{"input_tokens":1,"output_tokens":1,"cost":"1.5e-5"}}"#).unwrap();
        assert_eq!(seen.cost.as_deref(), Some("0.000015"));
        let seen = whole(r#"{"usage":{"input_tokens":1,"output_tokens":1,"cost":-4}}"#).unwrap();
        assert_eq!(seen.cost, None);
    }

    #[test]
    fn a_gateway_that_names_no_event_and_adds_comments_is_read_all_the_same() {
        let stream = concat!(
            ": OPENROUTER PROCESSING\n\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"g1\",\"model\":\"vendor/m\",\"usage\":{\"input_tokens\":8,\"output_tokens\":1}}}\n\n",
            ": keep-alive\n\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":4,\"cost\":0.00002}}\n\n",
        );
        let seen = whole(stream).unwrap();
        assert_eq!(seen.model.as_deref(), Some("vendor/m"));
        assert_eq!(
            (seen.tokens.input_tokens, seen.tokens.output_tokens),
            (8, 4)
        );
        assert_eq!(seen.cost.as_deref(), Some("0.00002"));
    }

    #[test]
    fn bytes_that_are_no_text_at_all_yield_nothing_and_do_no_harm() {
        let gzip = [0x1f_u8, 0x8b, 0x08, 0x00, 0xde, 0xad, 0xbe, 0xef];
        assert_eq!(feed(&[&gzip]).usage(), None);
        assert_eq!(feed(&[&[0xff, 0xfe, 0x00]]).usage(), None);
    }

    #[test]
    fn a_plain_answer_larger_than_the_bound_is_not_kept_and_not_read() {
        let mut observer = MessagesUsage::new();
        observer.observe(br#"{"content":""#);
        let filler = vec![b'a'; MAX_PLAIN_BODY_BYTES / 2];
        for _ in 0..3 {
            observer.observe(&filler);
        }
        observer.observe(br#""usage":{"input_tokens":1,"output_tokens":1}}"#);
        assert_eq!(observer.usage(), None);
        assert!(matches!(observer.body, Body::Plain(None)));
    }
}

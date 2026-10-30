//! Streaming usage observation.
//!
//! [`ResponsesUsage`] reads what the vendor says a response used, as the response passes by. The Responses
//! format answers in one of two ways and the observer tells them apart by the first byte it sees: a stream of
//! server-sent events (`event:` and `data:` lines, one `data:` document for each event) or one JSON document.
//!
//! In a stream the figures come once, in the `usage` member of the response that `response.completed` or
//! `response.incomplete` carries; the events that open the response (`response.created`,
//! `response.in_progress`) carry `usage: null` and are read for the response's id and the model that answers. A
//! stream that is cut before its last event therefore has no figures: the observer then says nothing about use
//! rather than zero, because nobody said it cost nothing. A plain answer, and the answer to a compaction
//! request, carry `usage` at the top. The members are `input_tokens` (which includes the tokens read from or
//! written to a cache, `input_tokens_details.cached_tokens` and `.cache_write_tokens`), `output_tokens` (which
//! includes `output_tokens_details.reasoning_tokens`) and `total_tokens`. Those are the conventions of the
//! model contract, so the counts are taken as they are and the total is recomputed as input plus output.
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
/// hundred thousand tokens, about a megabyte of text; eight leaves room for a compaction's echoed items.
pub const MAX_PLAIN_BODY_BYTES: usize = 8 * 1024 * 1024;

/// The events that carry a response object, and so possibly its figures.
const RESPONSE_EVENTS: [&str; 6] = [
    "response.created",
    "response.in_progress",
    "response.queued",
    "response.completed",
    "response.incomplete",
    "response.failed",
];

/// The members of a response object that matter here. Every other member, the output above all, is
/// skipped without being kept.
#[derive(Deserialize)]
struct ResponseView {
    id: Option<Value>,
    model: Option<Value>,
    usage: Option<Box<RawValue>>,
}

/// An event that carries a response object.
#[derive(Deserialize)]
struct EventView {
    response: Option<ResponseView>,
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
    #[serde(alias = "prompt_tokens")]
    input_tokens: Option<Value>,
    #[serde(alias = "completion_tokens")]
    output_tokens: Option<Value>,
    total_tokens: Option<Value>,
    #[serde(alias = "prompt_tokens_details")]
    input_tokens_details: Option<Value>,
    #[serde(alias = "completion_tokens_details")]
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

/// A member of a details object, as a count.
fn detail(details: Option<&Value>, member: &str) -> Option<i64> {
    count(details?.get(member))
}

/// The text of a string value.
fn text(value: Option<Value>) -> Option<String> {
    match value? {
        Value::String(text) if !text.is_empty() => Some(text),
        _ => None,
    }
}

/// The counts of a usage object, as the contract counts them.
fn tokens(usage: &UsageView) -> Option<TokenUsage> {
    let input = count(usage.input_tokens.as_ref());
    let output = count(usage.output_tokens.as_ref());
    let total = count(usage.total_tokens.as_ref());
    let (input, output) = match (input, output, total) {
        (Some(input), Some(output), _) => (input, output),
        (Some(input), None, Some(total)) => (input, (total - input).max(0)),
        (None, Some(output), Some(total)) => ((total - output).max(0), output),
        (Some(input), None, None) => (input, 0),
        (None, Some(output), None) => (0, output),
        (None, None, _) => return None,
    };
    let details = usage.input_tokens_details.as_ref();
    let cached = detail(details, "cached_tokens").unwrap_or(0).min(input);
    let written = detail(details, "cache_write_tokens")
        .unwrap_or(0)
        .min(input - cached);
    let reasoning = detail(usage.output_tokens_details.as_ref(), "reasoning_tokens")
        .unwrap_or(0)
        .min(output);
    Some(TokenUsage::from_parts(TokensReportedApart {
        input: input - cached - written,
        cache_read: cached,
        cache_write: written,
        output: output - reasoning,
        reasoning,
    }))
}

/// What has been learned of the response so far.
#[derive(Debug, Clone, Default)]
struct Seen {
    id: Option<String>,
    model: Option<String>,
    tokens: Option<TokenUsage>,
    cost: Option<String>,
}

impl Seen {
    /// Takes in a response object.
    fn take(&mut self, response: ResponseView) {
        if let Some(id) = text(response.id) {
            self.id = Some(id);
        }
        if let Some(model) = text(response.model) {
            self.model = Some(model);
        }
        let Some(raw) = response.usage else { return };
        let Ok(usage) = serde_json::from_str::<UsageView>(raw.get()) else {
            return;
        };
        if let Some(tokens) = tokens(&usage) {
            self.tokens = Some(tokens);
            self.cost = usage
                .cost
                .and_then(|cost| crate::decimal::plain(cost.get()));
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
        if !RESPONSE_EVENTS.contains(&kind) {
            return;
        }
        if let Ok(EventView {
            response: Some(response),
        }) = serde_json::from_str::<EventView>(&event.data)
        {
            self.take(response);
        }
    }

    /// What was seen, if it holds figures.
    fn observed(&self) -> Option<ObservedUsage> {
        Some(ObservedUsage {
            model: self.model.clone(),
            tokens: self.tokens?,
            cost: self.cost.clone(),
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

/// Reads the usage of one answer of the Responses format as it passes by.
#[derive(Debug, Default)]
pub struct ResponsesUsage {
    body: Body,
    seen: Seen,
    chat: bool,
}

impl ResponsesUsage {
    /// An observer that has seen nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Chat Completions puts the same response envelope directly in each SSE data frame.
    pub(crate) fn chat() -> Self {
        Self {
            chat: true,
            ..Self::default()
        }
    }
}

impl UsageObserver for ResponsesUsage {
    fn observe(&mut self, chunk: &[u8]) {
        let Self { body, seen, chat } = self;
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
                    if *chat {
                        if let Ok(response) = serde_json::from_str::<ResponseView>(&event.data) {
                            seen.take(response);
                        }
                    } else {
                        seen.take_event(&event);
                    }
                }
            }
        }
    }

    fn usage(&self) -> Option<ObservedUsage> {
        if let Body::Plain(Some(bytes)) = &self.body {
            let mut seen = self.seen.clone();
            if let Ok(response) = serde_json::from_slice::<ResponseView>(bytes) {
                seen.take(response);
            }
            return seen.observed();
        }
        self.seen.observed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_streams_and_plain_answers_report_the_same_inclusive_usage_without_rewriting() {
        let reply=br#"{"id":"chatcmpl-local","model":"local","choices":[],"usage":{"prompt_tokens":100,"completion_tokens":30,"total_tokens":130,"prompt_tokens_details":{"cached_tokens":20},"completion_tokens_details":{"reasoning_tokens":7},"cost":"0.002"}}"#;
        let stream = format!(
            "data: {{\"id\":\"chatcmpl-local\",\"model\":\"local\",\"choices\":[{{\"delta\":{{\"content\":\"Hello\"}}}}]}}\n\ndata: {}\n\ndata: [DONE]\n\n",
            String::from_utf8_lossy(reply)
        );
        let mut plain = ResponsesUsage::chat();
        for byte in reply {
            plain.observe(std::slice::from_ref(byte));
        }
        let mut streamed = ResponsesUsage::chat();
        for byte in stream.as_bytes() {
            streamed.observe(std::slice::from_ref(byte));
        }
        assert_eq!(plain.usage(), streamed.usage());
        let usage = plain.usage().unwrap();
        assert_eq!(usage.model.as_deref(), Some("local"));
        assert_eq!(usage.request_ref.as_deref(), Some("chatcmpl-local"));
        assert_eq!(usage.tokens.input_tokens, 100);
        assert_eq!(usage.tokens.output_tokens, 30);
        assert_eq!(usage.tokens.total_tokens, 130);
        assert_eq!(usage.tokens.cached_input_tokens, 20);
        assert_eq!(usage.tokens.reasoning_tokens, 7);
        assert_eq!(usage.cost.as_deref(), Some("0.002"));
    }

    #[test]
    fn a_chat_stream_cut_before_usage_does_not_invent_zero_cost() {
        let mut observer = ResponsesUsage::chat();
        observer.observe(b"data: {\"id\":\"chatcmpl-local\",\"choices\":[{\"delta\":{\"content\":\"Hello\"}}],\"usage\":null}\n\n");
        assert!(observer.usage().is_none());
    }

    /// A stream as the vendor writes one: a text answer with figures in the last event.
    const STREAM: &str = concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"sequence_number\":0,\"response\":{\"id\":\"resp_1\",\"object\":\"response\",\"created_at\":1,\"status\":\"in_progress\",\"model\":\"gpt-6-sol\",\"output\":[],\"usage\":null}}\n\n",
        "event: response.in_progress\n",
        "data: {\"type\":\"response.in_progress\",\"sequence_number\":1,\"response\":{\"id\":\"resp_1\",\"status\":\"in_progress\",\"model\":\"gpt-6-sol\",\"output\":[],\"usage\":null}}\n\n",
        "event: response.output_item.added\n",
        "data: {\"type\":\"response.output_item.added\",\"sequence_number\":2,\"output_index\":0,\"item\":{\"id\":\"msg_1\",\"type\":\"message\",\"status\":\"in_progress\",\"content\":[],\"role\":\"assistant\"}}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"sequence_number\":3,\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello\",\"logprobs\":[]}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"sequence_number\":4,\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\" there\",\"logprobs\":[]}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"sequence_number\":5,\"response\":{\"id\":\"resp_1\",\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-6-sol\",\"output\":[{\"id\":\"msg_1\",\"type\":\"message\",\"status\":\"completed\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello there\",\"annotations\":[],\"logprobs\":[]}]}],\"usage\":{\"input_tokens\":120,\"input_tokens_details\":{\"cached_tokens\":100},\"output_tokens\":30,\"output_tokens_details\":{\"reasoning_tokens\":10},\"total_tokens\":150}}}\n\n",
    );

    fn feed(chunks: &[&[u8]]) -> ResponsesUsage {
        let mut observer = ResponsesUsage::new();
        for chunk in chunks {
            observer.observe(chunk);
        }
        observer
    }

    fn whole(body: &str) -> Option<ObservedUsage> {
        feed(&[body.as_bytes()]).usage()
    }

    #[test]
    fn a_stream_reports_the_figures_of_its_last_event_with_the_model_and_the_response_id() {
        let seen = whole(STREAM).unwrap();
        assert_eq!(seen.model.as_deref(), Some("gpt-6-sol"));
        assert_eq!(seen.request_ref.as_deref(), Some("resp_1"));
        assert_eq!(seen.tokens.input_tokens, 120);
        assert_eq!(seen.tokens.cached_input_tokens, 100);
        assert_eq!(seen.tokens.output_tokens, 30);
        assert_eq!(seen.tokens.reasoning_tokens, 10);
        assert_eq!(seen.tokens.total_tokens, 150);
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
    fn a_stream_that_is_cut_before_its_last_event_says_nothing_about_use() {
        let cut = &STREAM[..STREAM.find("event: response.completed").unwrap()];
        assert_eq!(whole(cut), None, "nobody said it cost nothing");
        let half = &STREAM[..STREAM.len() - 40];
        assert_eq!(whole(half), None, "an event cut in two is no event");
    }

    #[test]
    fn what_a_stream_said_before_the_cut_is_still_there_when_the_figures_came_early() {
        // A gateway that reports figures on a first event and then goes on streaming.
        let early = concat!(
            "event: response.in_progress\n",
            "data: {\"type\":\"response.in_progress\",\"response\":{\"id\":\"r\",\"model\":\"m\",\"usage\":{\"input_tokens\":9,\"output_tokens\":0}}}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\n",
        );
        let seen = whole(early).unwrap();
        assert_eq!(
            (seen.tokens.input_tokens, seen.tokens.output_tokens),
            (9, 0)
        );
    }

    #[test]
    fn an_answer_that_stops_at_the_token_limit_still_reports_what_it_used() {
        let stream = concat!(
            "event: response.incomplete\n",
            "data: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"r\",\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"},\"model\":\"m\",\"output\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":64,\"total_tokens\":74}}}\n\n",
        );
        let seen = whole(stream).unwrap();
        assert_eq!(
            (seen.tokens.input_tokens, seen.tokens.output_tokens),
            (10, 64)
        );
    }

    #[test]
    fn a_failed_response_has_no_figures() {
        let stream = concat!(
            "event: response.failed\n",
            "data: {\"type\":\"response.failed\",\"response\":{\"id\":\"r\",\"status\":\"failed\",\"error\":{\"code\":\"server_error\",\"message\":\"x\"},\"usage\":null}}\n\n",
            "event: error\n",
            "data: {\"type\":\"error\",\"code\":\"server_error\",\"message\":\"x\",\"param\":null}\n\n",
        );
        assert_eq!(whole(stream), None);
    }

    #[test]
    fn a_plain_answer_is_read_from_its_usage_member_however_it_arrives() {
        let body = r#"{"id":"resp_2","object":"response","status":"completed","model":"gpt-6-sol","output":[{"type":"message","content":[{"type":"output_text","text":"{\"usage\":1}"}]}],"usage":{"input_tokens":12,"input_tokens_details":{"cached_tokens":0,"cache_write_tokens":4},"output_tokens":5,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":17}}"#;
        let expected = whole(body).unwrap();
        assert_eq!(expected.tokens.input_tokens, 12);
        assert_eq!(expected.tokens.total_tokens, 17);
        assert_eq!(expected.request_ref.as_deref(), Some("resp_2"));
        for size in [1, 5, 50] {
            let chunks: Vec<&[u8]> = body.as_bytes().chunks(size).collect();
            assert_eq!(
                feed(&chunks).usage(),
                Some(expected.clone()),
                "chunks of {size}"
            );
        }
    }

    #[test]
    fn the_answer_to_a_compaction_request_is_read_though_it_names_no_model() {
        let body = r#"{"id":"resp_c","object":"response.compaction","output":[],"created_at":1,"usage":{"input_tokens":900,"input_tokens_details":{"cached_tokens":0},"output_tokens":100,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":1000}}"#;
        let seen = whole(body).unwrap();
        assert_eq!(seen.model, None);
        assert_eq!(seen.tokens.total_tokens, 1000);
    }

    #[test]
    fn an_error_document_or_a_cut_document_says_nothing() {
        assert_eq!(
            whole(
                r#"{"error":{"message":"slow down","type":"rate_limit_error","param":null,"code":"slow_down"}}"#
            ),
            None
        );
        assert_eq!(whole(r#"{"id":"r","usage":{"input_tokens":5,"outp"#), None);
        assert_eq!(whole(""), None);
        assert_eq!(whole("   \n"), None);
    }

    #[test]
    fn the_total_is_input_plus_output_whatever_the_vendor_wrote() {
        let seen =
            whole(r#"{"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":999}}"#).unwrap();
        assert_eq!(seen.tokens.total_tokens, 15);
        assert!(seen.tokens.is_consistent());
        // A total and one side are enough to know the other.
        let seen = whole(r#"{"usage":{"output_tokens":5,"total_tokens":17}}"#).unwrap();
        assert_eq!(seen.tokens.input_tokens, 12);
    }

    #[test]
    fn a_part_cannot_be_larger_than_what_it_is_part_of() {
        let seen = whole(r#"{"usage":{"input_tokens":10,"input_tokens_details":{"cached_tokens":50},"output_tokens":5,"output_tokens_details":{"reasoning_tokens":9}}}"#).unwrap();
        assert_eq!(seen.tokens.cached_input_tokens, 10);
        assert_eq!(seen.tokens.reasoning_tokens, 5);
        assert!(seen.tokens.is_consistent());
    }

    #[test]
    fn counts_a_gateway_writes_loosely_are_still_counts() {
        let seen = whole(
            r#"{"usage":{"input_tokens":"42","output_tokens":7.0,"output_tokens_details":null}}"#,
        )
        .unwrap();
        assert_eq!(
            (seen.tokens.input_tokens, seen.tokens.output_tokens),
            (42, 7)
        );
        assert_eq!(
            whole(r#"{"usage":{"input_tokens":-3,"output_tokens":"many"}}"#),
            None
        );
        assert_eq!(whole(r#"{"usage":"none"}"#), None);
    }

    #[test]
    fn the_cost_a_gateway_states_is_kept_as_the_decimal_it_wrote() {
        for (written, kept) in [
            ("0.0000123", "0.0000123"),
            ("1.5e-5", "0.000015"),
            ("\"0.25\"", "0.25"),
            ("3", "3"),
            ("0", "0"),
            ("1E2", "100"),
        ] {
            let body =
                format!(r#"{{"usage":{{"input_tokens":1,"output_tokens":1,"cost":{written}}}}}"#);
            assert_eq!(
                whole(&body).unwrap().cost.as_deref(),
                Some(kept),
                "{written}"
            );
        }
        for written in ["-1", "\"free\"", "null", "true", "1e99"] {
            let body =
                format!(r#"{{"usage":{{"input_tokens":1,"output_tokens":1,"cost":{written}}}}}"#);
            assert_eq!(whole(&body).unwrap().cost, None, "{written}");
        }
    }

    #[test]
    fn a_gateway_that_names_no_event_and_adds_comments_is_read_all_the_same() {
        let stream = concat!(
            ": OPENROUTER PROCESSING\n\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r9\",\"model\":\"vendor/m\",\"usage\":null}}\n\n",
            ": keep-alive\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r9\",\"model\":\"vendor/m\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2,\"total_tokens\":5,\"cost\":0.00002}}}\n\n",
        );
        let seen = whole(stream).unwrap();
        assert_eq!(seen.model.as_deref(), Some("vendor/m"));
        assert_eq!(seen.tokens.total_tokens, 5);
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
        let mut observer = ResponsesUsage::new();
        observer.observe(br#"{"output":""#);
        let filler = vec![b'a'; MAX_PLAIN_BODY_BYTES / 2];
        for _ in 0..3 {
            observer.observe(&filler);
        }
        observer.observe(br#""usage":{"input_tokens":1,"output_tokens":1}}"#);
        assert_eq!(observer.usage(), None);
        assert!(matches!(observer.body, Body::Plain(None)));
    }
}

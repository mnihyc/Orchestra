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
use orch_model_contract::usage::{
    ObservedUsage, ResponseFacts, SseEvent, SseParser, UsageObserver,
};
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
    service_tier: Option<Value>,
    usage: Option<Box<RawValue>>,
    output: Option<Vec<SearchItem>>,
}

/// Only hosted search actions are counted; message bodies and function arguments are skipped.
#[derive(Deserialize)]
struct SearchItem {
    #[serde(rename = "type")]
    kind: Option<String>,
    status: Option<String>,
    action: Option<SearchAction>,
}

#[derive(Deserialize)]
struct SearchAction {
    #[serde(rename = "type")]
    kind: Option<String>,
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
    server_tool_use: Option<Value>,
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
fn write_parts(details: Option<&Value>) -> Option<(i64, i64)> {
    fn exact(value: &Value) -> Option<i64> {
        value
            .as_i64()
            .or_else(|| value.as_str()?.parse().ok())
            .filter(|value| *value >= 0)
    }
    let parts = details?.get("cache_write_tokens_details")?;
    Some((
        exact(parts.get("ttl_30m_tokens")?)?,
        exact(parts.get("ttl_12h_tokens")?)?,
    ))
}

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
        .or_else(|| {
            let (short, long) = write_parts(details)?;
            short.checked_add(long)
        })
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
    facts: ResponseFacts,
    declared: orch_model_contract::declared_usage::DeclaredUsage,
    cost_unit: Option<String>,
    web_searches: Option<i64>,
    id: Option<String>,
    model: Option<String>,
    service_tier: Option<String>,
    tokens: Option<TokenUsage>,
    cache_write_12h: Option<i64>,
    cost: Option<String>,
}

impl Seen {
    /// Takes in a response object.
    fn take(&mut self, response: ResponseView) {
        if let Some(output) = response.output {
            self.web_searches = Some(
                i64::try_from(
                    output
                        .iter()
                        .filter(|item| {
                            item.kind.as_deref() == Some("web_search_call")
                                && item.status.as_deref() == Some("completed")
                                && item
                                    .action
                                    .as_ref()
                                    .and_then(|action| action.kind.as_deref())
                                    == Some("search")
                        })
                        .count(),
                )
                .unwrap_or(i64::MAX),
            );
        }
        if let Some(tier) = text(response.service_tier) {
            self.service_tier = Some(tier);
        }
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
        if let Some(searches) = usage
            .server_tool_use
            .as_ref()
            .and_then(|usage| usage.get("tool_search"))
            .and_then(Value::as_i64)
            .filter(|count| *count >= 0)
        {
            // The server's aggregate is authoritative, including an explicit zero.
            self.web_searches = Some(searches);
        }
        if let Ok(value) = serde_json::from_str::<Value>(raw.get()) {
            self.declared.observe_usage(&value);
        }
        if let Some((actual, unit)) = self.declared.reported_cost(raw.get()) {
            self.cost = crate::decimal::plain(actual.get());
            self.cost_unit = unit;
        }
        if let Some(tokens) = tokens(&usage) {
            self.cache_write_12h = write_parts(usage.input_tokens_details.as_ref())
                .filter(|(short, long)| short.checked_add(*long) == Some(tokens.cache_write_tokens))
                .map(|(_, long)| long)
                .or_else(|| {
                    let details = usage.input_tokens_details.as_ref()?;
                    (details.get("cache_write_tokens_details").is_none()
                        && details.get("cache_write_tokens").is_some_and(|value| {
                            value.as_i64() == Some(0) || value.as_str() == Some("0")
                        })
                        && tokens.cache_write_tokens == 0)
                        .then_some(0)
                });
            self.tokens = Some(tokens);
            if !self.declared.has_reported_cost() {
                self.cost = usage
                    .cost
                    .and_then(|cost| crate::decimal::plain(cost.get()));
            }
        }
    }

    fn take_facts(&mut self, body: &Value) {
        let response = body.get("response").unwrap_or(body);
        if let Some(id) = response
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            self.facts.request_id = Some(id.to_owned());
        }
        let kind = body.get("type").and_then(Value::as_str).unwrap_or_default();
        self.facts.completed |= matches!(
            kind,
            "response.completed" | "response.incomplete" | "response.failed"
        ) || body.get("choices").and_then(Value::as_array).is_some_and(
            |choices| {
                choices.iter().any(|choice| {
                    choice
                        .get("finish_reason")
                        .is_some_and(|reason| !reason.is_null())
                })
            },
        );
        self.facts.output_started |= matches!(
            kind,
            "response.output_text.delta"
                | "response.reasoning_text.delta"
                | "response.reasoning_summary_text.delta"
                | "response.function_call_arguments.delta"
        ) && body
            .get("delta")
            .is_some_and(|delta| delta.as_str().is_some_and(|text| !text.is_empty()));
        if let Some(choices) = body.get("choices").and_then(Value::as_array) {
            self.facts.output_started |= choices.iter().any(|choice| {
                choice
                    .pointer("/delta/content")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty())
                    || choice
                        .pointer("/delta/tool_calls")
                        .and_then(Value::as_array)
                        .is_some_and(|calls| !calls.is_empty())
            });
        }
        if let Some(error) = response.get("error").filter(|error| !error.is_null()) {
            self.facts.failure = Some(serde_json::json!({"error":error}));
        }
        if let Some(choices) = body.get("choices").and_then(Value::as_array)
            && let Some(refusal) = choices
                .iter()
                .find_map(|choice| choice.pointer("/message/refusal").and_then(Value::as_str))
                .filter(|text| !text.is_empty())
        {
            self.facts.failure =
                Some(serde_json::json!({"error":{"type":"model_refused","message":refusal}}));
        }
        if let Some(output) = response.get("output").and_then(Value::as_array) {
            for item in output {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        if part["type"] == "refusal" {
                            self.facts.failure = Some(serde_json::json!({"error": {
                                "type":"model_refused", "message":part["refusal"]
                            }}));
                        }
                    }
                }
            }
        }
    }

    /// Takes in one event of a stream.
    fn take_event(&mut self, event: &SseEvent) {
        if let Ok(body) = serde_json::from_str::<Value>(&event.data) {
            self.take_facts(&body);
        }
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
        let mut observed = ObservedUsage {
            capacity_tier: None,
            cache_write_1h_tokens: None,
            cache_write_12h_tokens: self.cache_write_12h,
            web_searches: self.web_searches,
            declared_counters: Default::default(),
            cost_unit: self.cost_unit.clone(),
            web_fetches: None,
            service_tier: self.service_tier.clone(),
            model: self.model.clone(),
            tokens: self.tokens?,
            cost: self.cost.clone(),
            request_ref: self.id.clone(),
        };
        self.declared.apply(&mut observed);
        Some(observed)
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
    standalone: Option<StandaloneSearch>,
}

/// Request identity is used only after the same exchange proves a complete successful result.
#[derive(Debug, Clone, Default)]
struct StandaloneSearch {
    search: bool,
    page: bool,
    whole: bool,
    status: Option<u16>,
}

impl StandaloneSearch {
    fn of_request(body: &[u8]) -> Self {
        let commands = serde_json::from_slice::<Value>(body)
            .ok()
            .and_then(|body| body.get("commands").cloned());
        let nonempty = |key: &str| {
            commands
                .as_ref()
                .and_then(|commands| commands.get(key))
                .and_then(Value::as_array)
                .is_some_and(|values| !values.is_empty())
        };
        let search = nonempty("search_query");
        Self {
            search,
            page: !search && (nonempty("open") || nonempty("find")),
            ..Self::default()
        }
    }

    fn observed(&self, bytes: &[u8]) -> Option<ObservedUsage> {
        if !self.whole
            || !self
                .status
                .is_some_and(|status| (200..300).contains(&status))
        {
            return None;
        }
        let body: Value = serde_json::from_slice(bytes).ok()?;
        if body.get("error").is_some_and(|error| !error.is_null())
            || !body.get("output").is_some_and(Value::is_string)
            || body
                .get("results")
                .is_some_and(|value| !value.is_null() && !value.is_array())
            || body
                .get("encrypted_output")
                .is_some_and(|value| !value.is_null() && !value.is_string())
        {
            return None;
        }
        if !self.search && !self.page {
            return None;
        }
        Some(ObservedUsage {
            web_searches: Some(i64::from(self.search)),
            web_fetches: Some(i64::from(self.page)),
            ..ObservedUsage::default()
        })
    }
}

impl ResponsesUsage {
    /// An observer that has seen nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A side search shares this bounded plain-JSON reader, status and whole-response evidence.
    #[must_use]
    pub fn for_request(path: &str, body: &[u8]) -> Self {
        Self {
            standalone: (path.split('?').next() == Some("/alpha/search"))
                .then(|| StandaloneSearch::of_request(body)),
            ..Self::default()
        }
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
    fn response_status(&mut self, status: u16) {
        if let Some(search) = &mut self.standalone {
            search.status = Some(status);
        }
    }

    fn finish(&mut self, whole: bool) {
        if let Some(search) = &mut self.standalone {
            search.whole = whole;
        }
    }

    fn tokens_reported(&self) -> bool {
        self.standalone.is_none()
    }

    fn configure_capabilities(
        &mut self,
        bindings: &orch_model_contract::usage::NativeBindings,
    ) -> bool {
        self.seen.declared.configure(bindings);
        true
    }
    fn observe(&mut self, chunk: &[u8]) {
        let Self {
            body, seen, chat, ..
        } = self;
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
                        if event.data.trim() == "[DONE]" {
                            seen.facts.completed = true;
                        }
                        if let Ok(body) = serde_json::from_str::<Value>(&event.data) {
                            seen.take_facts(&body);
                        }
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
        if let Some(search) = &self.standalone {
            return match &self.body {
                Body::Plain(Some(bytes)) => search.observed(bytes),
                _ => None,
            };
        }
        if let Body::Plain(Some(bytes)) = &self.body {
            let mut seen = self.seen.clone();
            if let Ok(response) = serde_json::from_slice::<ResponseView>(bytes) {
                seen.take(response);
            }
            return seen.observed();
        }
        self.seen.observed()
    }
    fn response_facts(&self) -> ResponseFacts {
        let mut seen = self.seen.clone();
        if let Body::Plain(Some(bytes)) = &self.body
            && let Ok(body) = serde_json::from_slice::<Value>(bytes)
        {
            seen.take_facts(&body);
            seen.facts.completed = true;
        }
        seen.facts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standalone(commands: Value) -> ResponsesUsage {
        ResponsesUsage::for_request(
            "/alpha/search",
            serde_json::json!({
                "id":"native-session", "model":"opaque-model", "commands":commands,
                "settings":{"external_web_access":true}
            })
            .to_string()
            .as_bytes(),
        )
    }

    #[test]
    fn standalone_search_counts_only_a_whole_success_and_keeps_page_actions_separate()
    -> Result<(), Box<dyn std::error::Error>> {
        for (commands, searches, pages) in [
            (
                serde_json::json!({"search_query":[{"q":"one"},{"q":"two"}]}),
                Some(1),
                Some(0),
            ),
            (
                serde_json::json!({"open":[{"ref_id":"https://page.example.invalid"}]}),
                Some(0),
                Some(1),
            ),
            (
                serde_json::json!({"find":[{"ref_id":"page","pattern":"word"}]}),
                Some(0),
                Some(1),
            ),
            (
                serde_json::json!({"weather":[{"location":"London"}]}),
                None,
                None,
            ),
        ] {
            let mut reader = standalone(commands);
            reader.response_status(200);
            for byte in br#"{"output":"No results; failure is merely a word here","results":[]}"# {
                reader.observe(&[*byte]);
                assert!(
                    reader.usage().is_none(),
                    "a valid prefix does not prove body completion"
                );
            }
            reader.finish(true);
            if searches.is_none() && pages.is_none() {
                assert!(
                    reader.usage().is_none(),
                    "an unknown action declares no quantity"
                );
                continue;
            }
            let usage = reader
                .usage()
                .ok_or("a complete successful search result")?;
            assert_eq!(usage.web_searches, searches);
            assert_eq!(usage.web_fetches, pages);
            assert!(
                !reader.tokens_reported(),
                "this endpoint reported no generation tokens"
            );
        }
        Ok(())
    }

    #[test]
    fn standalone_errors_cuts_and_malformed_results_prove_no_paid_operation() {
        for (status, body, whole) in [
            (500, r#"{"output":"error page","results":[]}"#, true),
            (200, r#"{"error":{"message":"failed"},"output":"no"}"#, true),
            (200, r#"{"output":[],"results":[]}"#, true),
            (200, r#"{"output":"text","results":{}}"#, true),
            (200, r#"{"output":"text","encrypted_output":7}"#, true),
            (200, r#"{"output":"text"}trailing"#, true),
            (
                200,
                r#"{"output":"complete JSON, incomplete HTTP body"}"#,
                false,
            ),
            (200, r#"{"output":"cut"#, true),
        ] {
            let mut reader = standalone(serde_json::json!({"search_query":[{"q":"query"}]}));
            reader.response_status(status);
            reader.observe(body.as_bytes());
            reader.finish(whole);
            assert!(reader.usage().is_none(), "{status}/{whole}/{body}");
        }
    }

    #[test]
    fn cache_write_lifetimes_survive_plain_and_fragmented_terminal_usage() {
        let reply = serde_json::json!({"model":"cache-fixture","service_tier":"default",
            "usage":{"input_tokens":12000,"output_tokens":0,"input_tokens_details":{
                "cached_tokens":4000,"cache_write_tokens":7000,
                "cache_write_tokens_details":{"ttl_30m_tokens":3000,"ttl_12h_tokens":4000}}}});
        let bytes = serde_json::to_vec(&reply).unwrap();
        let mut plain = ResponsesUsage::new();
        plain.observe(&bytes);
        let event = format!(
            "event: response.completed\ndata: {}\n\n",
            serde_json::json!({"response":reply})
        );
        let mut streamed = ResponsesUsage::new();
        for byte in event.as_bytes() {
            streamed.observe(std::slice::from_ref(byte));
        }
        assert_eq!(plain.usage(), streamed.usage());
        let observed = plain.usage().unwrap();
        assert_eq!(observed.tokens.input_tokens, 12000);
        assert_eq!(observed.tokens.cached_input_tokens, 4000);
        assert_eq!(observed.tokens.cache_write_tokens, 7000);
        assert_eq!(observed.tokens.total_tokens, 12000);
        assert_eq!(observed.cache_write_12h_tokens, Some(4000));
    }

    #[test]
    fn cache_write_lifetime_zero_is_known_and_missing_or_inconsistent_parts_are_unknown() {
        for (parts, expected) in [
            (
                serde_json::json!({"ttl_30m_tokens":300,"ttl_12h_tokens":0}),
                Some(0),
            ),
            (
                serde_json::json!({"ttl_30m_tokens":0,"ttl_12h_tokens":300}),
                Some(300),
            ),
            (serde_json::json!({"ttl_30m_tokens":300}), None),
            (
                serde_json::json!({"ttl_30m_tokens":200,"ttl_12h_tokens":200}),
                None,
            ),
            (
                serde_json::json!({"ttl_30m_tokens":301,"ttl_12h_tokens":-1}),
                None,
            ),
            (
                serde_json::json!({"ttl_30m_tokens":299.5,"ttl_12h_tokens":0.5}),
                None,
            ),
            (serde_json::Value::Null, None),
        ] {
            let reply = serde_json::json!({"usage":{"input_tokens":1000,"output_tokens":0,
                "input_tokens_details":{"cache_write_tokens":300,"cache_write_tokens_details":parts}}});
            let mut observer = ResponsesUsage::new();
            observer.observe(&serde_json::to_vec(&reply).unwrap());
            let used = observer.usage().unwrap();
            assert_eq!(used.tokens.cache_write_tokens, 300);
            assert_eq!(used.cache_write_12h_tokens, expected);
        }
        let mut observer = ResponsesUsage::new();
        observer.observe(br#"{"usage":{"input_tokens":1,"output_tokens":0,"input_tokens_details":{"cache_write_tokens":0}}}"#);
        assert_eq!(observer.usage().unwrap().cache_write_12h_tokens, Some(0));
        let mut observer = ResponsesUsage::new();
        observer.observe(br#"{"usage":{"input_tokens":1,"output_tokens":0}}"#);
        assert_eq!(observer.usage().unwrap().cache_write_12h_tokens, None);
    }

    #[test]
    fn complete_lifetime_parts_supply_a_missing_aggregate_without_a_second_input_count() {
        let mut observer = ResponsesUsage::new();
        observer.observe(br#"{"usage":{"input_tokens":12000,"output_tokens":0,"input_tokens_details":{"cached_tokens":4000,"cache_write_tokens_details":{"ttl_30m_tokens":3000,"ttl_12h_tokens":4000}}}}"#);
        let used = observer.usage().unwrap();
        assert_eq!(used.tokens.input_tokens, 12000);
        assert_eq!(used.tokens.cache_write_tokens, 7000);
        assert_eq!(used.cache_write_12h_tokens, Some(4000));
    }

    #[test]
    fn output_evidence_exists_before_a_usage_body() {
        let mut observer = ResponsesUsage::new();
        observer
            .observe(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n");
        assert!(observer.response_facts().output_started);
        assert!(observer.usage().is_none());
    }

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

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod declared_counter_tests {
    use super::*;
    #[test]
    fn endpoint_counters_override_generic_attempts_and_keep_searches_and_fetches_apart() {
        let bindings: orch_model_contract::usage::NativeBindings =
            serde_json::from_value(serde_json::json!({"web_search":{"counters":{
            "web_searches":{"pointer":"/usage/server_tool_use/actual_searches","cumulative":true},
            "web_fetches":{"pointer":"/usage/server_tool_use/actual_fetches","cumulative":true}}}}))
            .unwrap();
        let mut observer = ResponsesUsage::new();
        observer.configure_capabilities(&bindings);
        let frame = serde_json::json!({"type":"response.completed","response":{"id":"fixture-response","model":"fixture-model","output":[{"type":"web_search_call","status":"completed","action":{"type":"search"}}],"usage":{"input_tokens":10,"output_tokens":2,"server_tool_use":{"tool_search":8,"actual_searches":3,"actual_fetches":1}}}});
        let bytes = format!("event: response.completed\ndata: {frame}\n\n");
        observer.observe(bytes.as_bytes());
        observer.observe(bytes.as_bytes());
        let seen = observer.usage().unwrap();
        assert_eq!(seen.web_searches, Some(3));
        assert_eq!(seen.web_fetches, Some(1));
        assert_eq!(seen.tokens.total_tokens, 12);
        assert_eq!(seen.declared_counters["web_searches"], Some(3));
    }
}

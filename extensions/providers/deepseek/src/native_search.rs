//! Explicit native search at DeepSeek's documented endpoint/model pairs.
//!
//! Input is a decoded protocol frame, not a second SSE parser or an HTTP client.
//! The caller keeps the ordinary model ceiling, item writer and ledger/pricer.

use orch_provider_contract::models::{ModelOffered, ModelSuggestion};
use orch_types::Json;
use serde_json::json;
use std::collections::BTreeMap;
use url::Url;

/// Documentation for the two compatible wire formats and their native counters.
pub const RESPONSES_DOCUMENTATION: &str = "https://api-docs.deepseek.com/guides/responses_api/";
/// The compatible endpoint's server-tool contract.
pub const ANTHROPIC_DOCUMENTATION: &str = "https://api-docs.deepseek.com/guides/anthropic_api/";
/// Native tools and the standalone service share successful-request prices.
pub const PRICING_DOCUMENTATION: &str = "https://api-docs.deepseek.com/guides/search/";
/// USD per successful search or fetch; currency conversion belongs to the common pricer.
pub const PER_CALL_USD: &str = "0.00075";

/// A documented endpoint identity; protocol compatibility alone supplies no vendor capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    /// Responses at the vendor's root base.
    Responses,
    /// Messages at the vendor's explicit compatible base.
    Anthropic,
}

/// Match the actual protocol and exact vendor base, including its trust boundary.
#[must_use]
pub fn endpoint(protocol: &str, base: &Url) -> Option<Endpoint> {
    if !crate::known_vendor(base) {
        return None;
    }
    match (protocol, base.path()) {
        ("openai-responses", "" | "/") => Some(Endpoint::Responses),
        ("anthropic-messages", "/anthropic" | "/anthropic/") => Some(Endpoint::Anthropic),
        _ => None,
    }
}

/// Discovery offers abilities without enabling a mode or changing a manager's default.
/// Unknown models remain unknown; Flash explicitly offers no native search.
#[must_use]
pub fn offered(protocol: &str, base: &Url, model: &str) -> Option<ModelOffered> {
    endpoint(protocol, base)?;
    let modes = match model {
        "deepseek-v4-pro" => vec!["live".to_owned()],
        "deepseek-flash" => Vec::new(),
        _ => return None,
    };
    Some(ModelOffered {
        web_search: Some(modes),
        ..Default::default()
    })
}

/// Add only this source's actual offered search modes to the existing suggestion.
/// Enabled choices, defaults, rates and unrelated observations are untouched.
pub fn enrich(protocol: &str, base: &Url, suggestion: &mut ModelSuggestion) {
    if suggestion
        .parameters
        .offered
        .as_ref()
        .and_then(|offered| offered.web_search.as_ref())
        .is_none()
        && let Some(actual) = offered(protocol, base, &suggestion.model_name)
    {
        suggestion
            .parameters
            .offered
            .get_or_insert_with(ModelOffered::default)
            .web_search = actual.web_search;
        suggestion
            .origins
            .insert("/parameters/offered/web_search".into(), "catalogue".into());
    }
}

/// Data for the existing WebSearchDeclaration builder, under this exact endpoint/model mapping.
/// No cached mode or guessed compatible endpoint is declared.
#[must_use]
pub fn declaration(protocol: &str, base: &Url, model: &str) -> Option<Json> {
    if model != "deepseek-v4-pro" {
        return None;
    }
    let (paths, tool, identities, schema) = match endpoint(protocol, base)? {
        Endpoint::Responses => (
            json!(["/responses"]),
            json!({"type":"web_search"}),
            json!([{"type":"web_search"}]),
            json!({"type":"object","properties":{},"additionalProperties":false}),
        ),
        Endpoint::Anthropic => (
            json!(["/v1/messages"]),
            json!({"type":"web_search_20250305","name":"web_search"}),
            json!([{"type":"web_search_20250305","name":"web_search"},{"type":"web_search_20260209","name":"web_search"}]),
            json!({"type":"object","properties":{"max_uses":{"type":["integer","null"],"minimum":0}},"additionalProperties":false}),
        ),
    };
    Some(
        json!({"options_pointer":"","paths":paths,"path_suffixes":[],"tool":tool,
        "identities":identities,"modes":{"live":{}},"options_schema":schema}),
    )
}

/// A price quote in its original unit, never an assumed number of Orch credits.
#[must_use]
pub fn prices() -> Json {
    json!({"unit":"USD","web_search_per_call":PER_CALL_USD,"web_fetch_per_call":PER_CALL_USD})
}

/// Successful cumulative request counters. Unknown, zero and attempts stay distinct.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Usage {
    /// Billable successful search queries, not displayed item count.
    pub searches: Option<i64>,
    /// Billable successful page reads, where this endpoint actually tells them.
    pub fetches: Option<i64>,
    /// Attempts may exceed the configured max_uses and must never price as successes.
    pub attempted_searches: Option<i64>,
}
fn count(value: Option<&Json>) -> Option<i64> {
    value.and_then(Json::as_i64).filter(|value| *value >= 0)
}
impl Usage {
    /// Read a complete usage snapshot. This never adds an initial and final cumulative report.
    #[must_use]
    pub fn from_usage(endpoint: Endpoint, usage: &Json) -> Self {
        let Some(usage) = usage.as_object() else {
            return Self::default();
        };
        if count(usage.get("output_tokens")).is_none()
            || (endpoint == Endpoint::Responses && count(usage.get("input_tokens")).is_none())
        {
            return Self::default();
        }
        let tools = usage.get("server_tool_use");
        match endpoint {
            Endpoint::Responses => match tools {
                None => Self {
                    searches: Some(0),
                    fetches: Some(0),
                    attempted_searches: None,
                },
                Some(tools) => Self {
                    searches: count(tools.get("tool_search")),
                    fetches: count(tools.get("tool_fetch")),
                    attempted_searches: None,
                },
            },
            Endpoint::Anthropic => match tools {
                None => Self {
                    searches: Some(0),
                    fetches: None,
                    attempted_searches: Some(0),
                },
                Some(tools) => Self {
                    searches: count(tools.get("tool_search")),
                    fetches: count(tools.get("tool_fetch")),
                    attempted_searches: count(tools.get("web_search_requests")),
                },
            },
        }
    }
}

/// One native server item for the existing timeline writer. No client tool needs execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The native call identity, shared with its result block.
    pub id: String,
    /// The existing web_search or web_fetch presentation family.
    pub kind: &'static str,
    /// Native progress, completed or failed state.
    pub state: String,
    /// Factual query/queries, URL/reference and pattern only.
    pub input: Json,
    /// The existing WebSearch presentation JSON, once its real query is known.
    pub presentation: Option<Json>,
    /// Safe result metadata. A caller stores any returned page text through its normal OutputRef path.
    pub output: Option<Json>,
}
#[derive(Debug, Clone)]
struct Call {
    item: Item,
    partial: String,
    invalid_input: bool,
}
impl Call {
    fn new(id: &str) -> Self {
        Self {
            item: Item {
                id: id.into(),
                kind: "web_search",
                state: "in_progress".into(),
                input: json!({}),
                presentation: None,
                output: None,
            },
            partial: String::new(),
            invalid_input: false,
        }
    }
    fn input(&mut self, value: Json) {
        let mut input = serde_json::Map::new();
        for name in ["type", "query", "queries", "url", "ref_id", "pattern"] {
            if let Some(value) = value.get(name) {
                input.insert(name.into(), value.clone());
            }
        }
        self.item.input = Json::Object(input);
        self.refresh();
    }
    fn refresh(&mut self) {
        if self.item.kind != "web_search" {
            self.item.presentation = None;
            return;
        }
        let query = self
            .item
            .input
            .get("query")
            .and_then(Json::as_str)
            .map(str::to_owned)
            .or_else(|| {
                self.item
                    .input
                    .get("queries")
                    .and_then(Json::as_array)
                    .filter(|queries| !queries.is_empty())
                    .and_then(|queries| {
                        queries
                            .iter()
                            .map(Json::as_str)
                            .collect::<Option<Vec<_>>>()
                            .map(|queries| queries.join("\n"))
                    })
            });
        self.item.presentation = query.map(|query|json!({"kind":"web_search","query":query,
            "results":self.item.output.as_ref().and_then(|output|output.get("results")).cloned().unwrap_or_else(||json!([]))}));
    }
    fn results(&mut self, values: &Json) {
        let results = values
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|result| {
                let url = result.get("url")?.as_str()?;
                let title = result.get("title").and_then(Json::as_str).unwrap_or(url);
                Some(json!({"title":title,"url":url}))
            })
            .collect::<Vec<_>>();
        self.item.output = Some(json!({"results":results}));
        self.refresh();
    }
    fn response_item(&mut self, item: &Json) {
        if let Some(state) = item.get("status").and_then(Json::as_str) {
            self.item.state = state.into();
        }
        if let Some(action) = item.get("action") {
            match action.get("type").and_then(Json::as_str) {
                Some("search") => {
                    self.item.kind = "web_search";
                    self.input(action.clone());
                    if let Some(results) = action.get("sources").or_else(|| item.get("results")) {
                        self.results(results);
                    }
                }
                Some("open_page" | "find_in_page") => {
                    self.item.kind = "web_fetch";
                    self.input(action.clone());
                }
                _ => {}
            }
        }
    }
}

/// Vendor mapping over the caller's existing decoded stream. Raw frames remain unchanged.
/// The stream belongs to one real request; it supplies no HTTP transport or admission.
#[derive(Debug)]
pub struct Stream {
    endpoint: Endpoint,
    calls: BTreeMap<String, Call>,
    blocks: BTreeMap<u64, String>,
    usage: Usage,
}
impl Stream {
    /// Construct only for a supported exact endpoint/model pair.
    #[must_use]
    pub fn new(protocol: &str, base: &Url, model: &str) -> Option<Self> {
        declaration(protocol, base, model)?;
        Some(Self {
            endpoint: endpoint(protocol, base)?,
            calls: BTreeMap::new(),
            blocks: BTreeMap::new(),
            usage: Usage::default(),
        })
    }
    /// Counts actually told so far; initial null usage never becomes an invented zero.
    #[must_use]
    pub fn usage(&self) -> &Usage {
        &self.usage
    }
    /// Map only this vendor's native server-tool frames. Client tool calls pass unchanged.
    pub fn frame(&mut self, event: &str, value: &Json) -> Vec<Item> {
        if !event.is_empty()
            && value
                .get("type")
                .and_then(Json::as_str)
                .is_some_and(|kind| kind != event)
        {
            return Vec::new();
        }
        match self.endpoint {
            Endpoint::Responses => self.responses(event, value),
            Endpoint::Anthropic => self.messages(event, value),
        }
    }
    fn responses(&mut self, event: &str, value: &Json) -> Vec<Item> {
        let event = if event.is_empty() {
            value.get("type").and_then(Json::as_str).unwrap_or("")
        } else {
            event
        };
        if matches!(
            event,
            "response.completed" | "response.incomplete" | "response.failed"
        ) {
            if let Some(usage) = value
                .pointer("/response/usage")
                .filter(|usage| usage.is_object())
            {
                self.usage = Usage::from_usage(self.endpoint, usage);
            }
            return Vec::new();
        }
        if matches!(
            event,
            "response.output_item.added" | "response.output_item.done"
        ) {
            let Some(item) = value
                .get("item")
                .filter(|item| item.get("type").and_then(Json::as_str) == Some("web_search_call"))
            else {
                return Vec::new();
            };
            let Some(id) = item.get("id").and_then(Json::as_str) else {
                return Vec::new();
            };
            let call = self.calls.entry(id.into()).or_insert_with(|| Call::new(id));
            call.response_item(item);
            return vec![call.item.clone()];
        }
        let state = match event {
            "response.web_search_call.in_progress" => "in_progress",
            "response.web_search_call.searching" => "searching",
            "response.web_search_call.completed" => "completed",
            _ => return Vec::new(),
        };
        let Some(id) = value.get("item_id").and_then(Json::as_str) else {
            return Vec::new();
        };
        let call = self.calls.entry(id.into()).or_insert_with(|| Call::new(id));
        call.item.state = state.into();
        vec![call.item.clone()]
    }
    fn messages(&mut self, event: &str, value: &Json) -> Vec<Item> {
        let event = if event.is_empty() {
            value.get("type").and_then(Json::as_str).unwrap_or("")
        } else {
            event
        };
        if event == "message_delta" {
            if let Some(usage) = value.get("usage").filter(|usage| usage.is_object()) {
                self.usage = Usage::from_usage(self.endpoint, usage);
            }
            return Vec::new();
        }
        if event == "content_block_start" {
            if let Some(index) = value.get("index").and_then(Json::as_u64) {
                self.blocks.remove(&index);
            }
            let Some(block) = value.get("content_block") else {
                return Vec::new();
            };
            match block.get("type").and_then(Json::as_str) {
                Some("server_tool_use")
                    if block.get("name").and_then(Json::as_str) == Some("web_search") =>
                {
                    let (Some(id), Some(index)) = (
                        block.get("id").and_then(Json::as_str),
                        value.get("index").and_then(Json::as_u64),
                    ) else {
                        return Vec::new();
                    };
                    self.blocks.insert(index, id.into());
                    let call = self.calls.entry(id.into()).or_insert_with(|| Call::new(id));
                    if let Some(input) = block.get("input") {
                        call.input(input.clone());
                    }
                    return vec![call.item.clone()];
                }
                Some("web_search_tool_result") => {
                    let Some(id) = block.get("tool_use_id").and_then(Json::as_str) else {
                        return Vec::new();
                    };
                    let Some(call) = self.calls.get_mut(id) else {
                        return Vec::new();
                    };
                    if let Some(content) = block.get("content") {
                        if let Some(values) = content.as_array() {
                            call.results(content);
                            let errors = values
                                .iter()
                                .filter(|value| {
                                    value.get("type").and_then(Json::as_str)
                                        == Some("web_search_tool_result_error")
                                })
                                .collect::<Vec<_>>();
                            if errors.is_empty() {
                                call.item.state = "completed".into();
                            } else {
                                call.item.state = "failed".into();
                                if let Some(output) =
                                    call.item.output.as_mut().and_then(Json::as_object_mut)
                                {
                                    output.insert(
                                        "errors".into(),
                                        json!(
                                            errors
                                                .iter()
                                                .filter_map(|value| value
                                                    .get("error_code")
                                                    .and_then(Json::as_str))
                                                .collect::<Vec<_>>()
                                        ),
                                    );
                                    output.insert(
                                        "error".into(),
                                        json!(
                                            errors
                                                .first()
                                                .and_then(|error| error.get("error_code"))
                                                .and_then(Json::as_str)
                                        ),
                                    );
                                }
                            }
                        } else if content.get("type").and_then(Json::as_str)
                            == Some("web_search_tool_result_error")
                        {
                            call.item.state = "failed".into();
                            call.item.output = Some(
                                json!({"error":content.get("error_code").and_then(Json::as_str)}),
                            );
                        }
                    }
                    return vec![call.item.clone()];
                }
                _ => return Vec::new(),
            }
        }
        let Some(index) = value.get("index").and_then(Json::as_u64) else {
            return Vec::new();
        };
        let id = if event == "content_block_stop" {
            self.blocks.remove(&index)
        } else {
            self.blocks.get(&index).cloned()
        };
        let Some(id) = id else {
            return Vec::new();
        };
        let Some(call) = self.calls.get_mut(&id) else {
            return Vec::new();
        };
        if event == "content_block_delta"
            && value.pointer("/delta/type").and_then(Json::as_str) == Some("input_json_delta")
        {
            if let Some(partial) = value.pointer("/delta/partial_json").and_then(Json::as_str) {
                // Reuse the provider envelope bound for one retained native argument object.
                if call.partial.len().saturating_add(partial.len())
                    > orch_provider_contract::limits::REPORT_MAX_BYTES
                {
                    call.partial.clear();
                    call.invalid_input = true;
                } else if !call.invalid_input {
                    call.partial.push_str(partial);
                }
            }
        } else if event == "content_block_stop" && !call.invalid_input && !call.partial.is_empty() {
            if let Ok(input) = serde_json::from_str::<Json>(&call.partial) {
                call.input(input);
            }
            call.partial.clear();
            return vec![call.item.clone()];
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests;

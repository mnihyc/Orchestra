//! Usage comes from the original response and preserves missing grounding counts as unknown.
use orch_agent_contract::events::TokenUsage;
use orch_model_contract::declared_usage::DeclaredUsage;
use orch_model_contract::usage::{
    NativeBindings, ObservedUsage, ResponseFacts, SseParser, UsageObserver,
};
use orch_types::Json;

#[derive(Default)]
pub struct GoogleUsage {
    parser: SseParser,
    plain: Vec<u8>,
    is_plain: Option<bool>,
    seen: Option<ObservedUsage>,
    tokens_seen: bool,
    facts: ResponseFacts,
    model: Option<String>,
    service_tier: Option<String>,
    declared: DeclaredUsage,
}

impl GoogleUsage {
    fn read(&mut self, value: &Json) {
        let resource = value.get("interaction").unwrap_or(value);
        if let Some(model) = resource.get("model").and_then(Json::as_str) {
            self.model = Some(model.into());
        }
        if let Some(tier) = resource.get("service_tier").and_then(Json::as_str) {
            self.service_tier = Some(tier.into());
        }
        if let Some(seen) = self.seen.as_mut() {
            seen.model = self.model.clone();
            seen.service_tier = self.service_tier.clone().or(seen.service_tier.clone());
            seen.request_ref = self.facts.request_id.clone();
        }
        if let Some(id) = resource.get("id").and_then(Json::as_str) {
            self.facts.request_id = Some(id.into());
        }
        self.facts.output_started |= value
            .pointer("/delta/text")
            .and_then(Json::as_str)
            .is_some_and(|text| !text.is_empty());
        if value["event_type"] == "interaction.completed"
            || matches!(
                resource.get("status").and_then(Json::as_str),
                Some("completed" | "requires_action" | "incomplete")
            )
        {
            self.facts.completed = true;
        }
        if let Some(error) = resource.get("error").filter(|error| !error.is_null()) {
            self.facts.failure = Some(error.clone());
        }
        let Some(usage) = resource
            .get("usage")
            .or_else(|| value.pointer("/metadata/total_usage"))
        else {
            return;
        };
        self.declared.observe_usage(usage);
        let count = |key| {
            usage
                .get(key)
                .and_then(Json::as_i64)
                .filter(|count| *count >= 0)
        };
        let previous = self.seen.clone().unwrap_or_default();
        let input = count("total_input_tokens").unwrap_or(previous.tokens.input_tokens);
        let reasoning = count("total_thought_tokens").unwrap_or(previous.tokens.reasoning_tokens);
        let output = count("total_output_tokens").map_or(previous.tokens.output_tokens, |output| {
            output.saturating_add(reasoning)
        });
        self.tokens_seen |= [
            "total_input_tokens",
            "total_output_tokens",
            "total_thought_tokens",
            "total_tokens",
            "total_cached_tokens",
        ]
        .iter()
        .any(|key| count(key).is_some());
        let grounding = grounding_count(usage);
        let mut observed = ObservedUsage {
            model: self.model.clone().or(previous.model),
            request_ref: self.facts.request_id.clone(),
            service_tier: resource
                .get("service_tier")
                .or_else(|| usage.get("service_tier"))
                .and_then(Json::as_str)
                .map(str::to_owned)
                .or(self.service_tier.clone())
                .or(previous.service_tier),
            web_searches: grounding.or(previous.web_searches),
            tokens: TokenUsage {
                input_tokens: input,
                output_tokens: output,
                total_tokens: input.saturating_add(output),
                cached_input_tokens: count("total_cached_tokens")
                    .unwrap_or(previous.tokens.cached_input_tokens),
                reasoning_tokens: reasoning,
                ..Default::default()
            },
            ..Default::default()
        };
        self.declared.apply(&mut observed);
        self.seen = Some(observed);
    }
}

impl UsageObserver for GoogleUsage {
    fn configure_capabilities(&mut self, bindings: &NativeBindings) -> bool {
        self.declared.configure(bindings);
        true
    }
    fn observe(&mut self, bytes: &[u8]) {
        if self.is_plain.is_none() {
            self.is_plain = bytes
                .iter()
                .find(|byte| !byte.is_ascii_whitespace())
                .map(|byte| *byte == b'{');
        }
        if self.is_plain == Some(true) {
            self.plain.extend_from_slice(bytes);
            return;
        }
        for event in self.parser.push(bytes) {
            if let Ok(value) = serde_json::from_str(&event.data) {
                self.read(&value);
            }
        }
    }
    fn finish(&mut self, whole: bool) {
        if whole
            && self.is_plain == Some(true)
            && let Ok(value) = serde_json::from_slice(&self.plain)
        {
            self.read(&value);
        }
        self.plain.clear();
    }
    fn tokens_reported(&self) -> bool {
        self.tokens_seen
    }
    fn usage(&self) -> Option<ObservedUsage> {
        self.seen.clone()
    }
    fn response_facts(&self) -> ResponseFacts {
        self.facts.clone()
    }
}

fn grounding_count(usage: &Json) -> Option<i64> {
    let counts = usage.get("grounding_tool_count")?.as_array()?;
    let mut found = false;
    let mut total = 0_i64;
    for item in counts.iter().filter(|item| item["type"] == "google_search") {
        let count = item.get("count")?.as_i64().filter(|count| *count >= 0)?;
        total = total.saturating_add(count);
        found = true;
    }
    found.then_some(total)
}

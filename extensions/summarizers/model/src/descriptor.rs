//! Configuration schema and supported fields.
//!
//! The configuration is the rest of auto.summarizer after its kind, with bounded activity defaults.

use orch_link_contract::commands::{CompletionPurpose, ProviderComplete};
use orch_summarizer_contract::descriptor::{SummarizerDescriptor, SummaryField};
use orch_summarizer_contract::summarizer::SummarizeError;
use orch_types::{Id, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A paragraph's ordinary answer allowance, separate from its character target and reasoning room.
pub const ANSWER_OUTPUT_TOKENS: i64 = 1_024;
/// Titles stay short even when paragraph answers have more room.
pub const TITLE_OUTPUT_TOKENS: i64 = 120;
/// A goal verdict and the evidence/reason that accompanies it.
pub const GOAL_OUTPUT_TOKENS: i64 = 512;
/// The room a model that reasons is left for its reasoning, beyond the answer's own allowance.
pub const REASONING_TOKENS: i64 = 2_000;
/// Ordinary input planning budget; an explicitly larger budget may use the model's available window.
pub const CONTEXT_TOKENS: u32 = 32_000;

const fn output_tokens() -> i64 {
    ANSWER_OUTPUT_TOKENS
}
const fn title_output_tokens() -> i64 {
    TITLE_OUTPUT_TOKENS
}
const fn goal_output_tokens() -> i64 {
    GOAL_OUTPUT_TOKENS
}
const fn reasoning_tokens() -> i64 {
    REASONING_TOKENS
}
const fn context_tokens() -> u32 {
    CONTEXT_TOKENS
}

/// Configuration of the built-in model producer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The API provider whose key pays.
    pub provider_id: Id,
    /// Its offered model's routing name.
    pub model_name: String,
    /// Model request options in the selected endpoint's format; the host applies current model defaults.
    #[serde(default = "empty_params")]
    pub params: Json,
    /// Answer tokens for paragraphs, descriptions, tags and handoffs; independent of target characters.
    #[serde(default = "output_tokens")]
    pub answer_max_output_tokens: i64,
    /// Answer tokens for titles, independent of the title's character target.
    #[serde(default = "title_output_tokens")]
    pub title_max_output_tokens: i64,
    /// Answer tokens for a goal verdict and its reason; no hidden minimum overrides this choice.
    #[serde(default = "goal_output_tokens")]
    pub goal_max_output_tokens: i64,
    /// Reasoning room beyond the answer's allowance, also reserved when upstream reasoning is unrecorded.
    /// A model explicitly reported without thinking support gets none. Zero leaves no room.
    #[serde(default = "reasoning_tokens")]
    pub reasoning_tokens: i64,
    /// Input token budget, including the instruction and frame; bounded by the model window less output.
    #[serde(default = "context_tokens")]
    pub context_tokens: u32,
    /// Replacement instructions by field; null keeps the built-in text.
    #[serde(default)]
    pub prompts: BTreeMap<String, Option<String>>,
}

fn empty_params() -> Json {
    serde_json::json!({})
}

impl Config {
    /// One existing completion shape for every summary field and goal verification.
    #[must_use]
    pub fn completion(
        &self,
        system: String,
        input: String,
        max_output_tokens: i64,
    ) -> ProviderComplete {
        ProviderComplete {
            model_parameters: None,
            provider_id: self.provider_id,
            purpose: CompletionPurpose::Summary,
            // The host selects a protocol offered by this actual provider.
            protocol: String::new(),
            model_name: self.model_name.clone(),
            params: Some(self.params.clone()),
            system,
            input,
            max_output_tokens,
        }
    }

    /// The completion of a model with what its parameters say of reasoning: a model that reasons is left room
    /// for it beyond `answer_tokens`, and is asked for the lowest effort it offers unless `params` choose one.
    #[must_use]
    pub fn completion_with(
        &self,
        reasoning: &crate::completion::Reasoning,
        system: String,
        input: String,
        answer_tokens: i64,
    ) -> ProviderComplete {
        let mut request = self.completion(
            system,
            input,
            answer_tokens.saturating_add(self.reasoning_room(reasoning.reasons)),
        );
        if reasoning.reasons
            && let Some(effort) = &reasoning.lowest_effort
            && let Some(params) = request.params.as_mut()
            && !orch_provider_contract::calls::names_effort(params)
        {
            params["effort"] = serde_json::json!(effort);
        }
        request
    }

    /// The output a request may spend: the answer's allowance and, for a model that reasons, its room.
    #[must_use]
    pub fn output_reserve(&self, field: SummaryField, reasons: bool) -> i64 {
        self.answer_tokens(field)
            .saturating_add(self.reasoning_room(reasons))
    }

    /// The selected task's answer allowance, before any reasoning room is added.
    #[must_use]
    pub fn answer_tokens(&self, field: SummaryField) -> i64 {
        if field == SummaryField::Title {
            self.title_max_output_tokens
        } else {
            self.answer_max_output_tokens
        }
    }

    /// The goal verifier leaves the same reasoning room beyond its independently chosen answer.
    #[must_use]
    pub fn goal_output_reserve(&self, reasons: bool) -> i64 {
        self.goal_max_output_tokens
            .saturating_add(self.reasoning_room(reasons))
    }

    fn reasoning_room(&self, reasons: bool) -> i64 {
        if reasons { self.reasoning_tokens } else { 0 }
    }

    /// Reads the schema's configuration and refuses an empty name or nonpositive budget.
    pub fn read(value: &Json) -> Result<Self, SummarizeError> {
        let config: Self = serde_json::from_value(value.clone()).map_err(|_| {
            SummarizeError::Config("the selected model configuration does not fit".to_owned())
        })?;
        if config.model_name.trim().is_empty()
            || config.answer_max_output_tokens <= 0
            || config.title_max_output_tokens <= 0
            || config.goal_max_output_tokens <= 0
            || config.reasoning_tokens < 0
            || config.context_tokens == 0
        {
            return Err(SummarizeError::Config(
                "model name and token budgets must be nonempty and positive".to_owned(),
            ));
        }
        if !config.params.is_object() {
            return Err(SummarizeError::Config(
                "model request options must be an object".to_owned(),
            ));
        }
        Ok(config)
    }
}

/// The built-in producer's declaration, including the roll-up and handoff operations.
#[must_use]
pub fn descriptor() -> SummarizerDescriptor {
    let mut schema = schemars::schema_for!(Config).to_value();
    // Match Config::read's positive budgets and nonblank model name at the save boundary too.
    for name in [
        "answer_max_output_tokens",
        "title_max_output_tokens",
        "goal_max_output_tokens",
        "context_tokens",
    ] {
        schema["properties"][name]["minimum"] = serde_json::json!(1);
    }
    schema["properties"]["reasoning_tokens"]["minimum"] = serde_json::json!(0);
    schema["properties"]["model_name"]["pattern"] = serde_json::json!("\\S");
    schema["properties"]["params"] = serde_json::json!({
        "type": "object",
        "additionalProperties": true,
        "description": "Model request options in this endpoint's format. Current model defaults apply when omitted; routing, prompt, tools and credentials remain owned by Orch."
    });
    let mut descriptor = SummarizerDescriptor {
        name: "model".to_owned(),
        title: "A model".to_owned(),
        config_schema: schema,
        fields: SummaryField::ALL.to_vec(),
        extensions: Default::default(),
    };
    // This fixed x_ member belongs to the descriptor's validated extension namespace.
    let _ = descriptor.extensions.insert(
        "x_capability_declarations",
        serde_json::json!(crate::capabilities::declarations()),
    );
    descriptor
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_model_that_reasons_is_left_room_and_asked_for_its_lowest_effort_unless_params_choose()
    -> Result<(), SummarizeError> {
        use crate::completion::Reasoning;
        let id = orch_types::new_id();
        let config = Config::read(&json!({"provider_id":id,"model_name":"small"}))?;
        let reasons = Reasoning {
            reasons: true,
            lowest_effort: Some("low".to_owned()),
        };
        let asked = config.completion_with(&reasons, "s".into(), "i".into(), 120);
        assert_eq!(asked.max_output_tokens, 120 + REASONING_TOKENS);
        assert_eq!(asked.params, Some(json!({"effort":"low"})));
        let unknown = config.completion_with(&Reasoning::default(), "s".into(), "i".into(), 120);
        assert_eq!(unknown.max_output_tokens, 120 + REASONING_TOKENS);
        assert_eq!(unknown.params, Some(json!({})));
        let plain = config.completion_with(
            &Reasoning {
                reasons: false,
                lowest_effort: None,
            },
            "s".into(),
            "i".into(),
            120,
        );
        assert_eq!(plain.max_output_tokens, 120);
        assert_eq!(plain.params, Some(json!({})));
        let chosen = Config::read(&json!({
            "provider_id":id,"model_name":"small","params":{"effort":"high"},"reasoning_tokens":500
        }))?;
        let asked = chosen.completion_with(&reasons, "s".into(), "i".into(), 120);
        assert_eq!(asked.max_output_tokens, 620);
        assert_eq!(asked.params, Some(json!({"effort":"high"})));
        Ok(())
    }

    #[test]
    fn input_and_each_answer_task_have_positive_independent_token_defaults()
    -> Result<(), SummarizeError> {
        let value = json!({"provider_id":orch_types::new_id(),"model_name":"small"});
        let config = Config::read(&value)?;
        assert_eq!(config.context_tokens, 32_000);
        assert_eq!(config.answer_max_output_tokens, ANSWER_OUTPUT_TOKENS);
        assert_eq!(config.title_max_output_tokens, TITLE_OUTPUT_TOKENS);
        assert_eq!(config.goal_max_output_tokens, GOAL_OUTPUT_TOKENS);
        let schema = descriptor().config_schema;
        assert_eq!(
            schema["properties"]["context_tokens"]["default"],
            json!(32_000)
        );
        assert!(schema["properties"].get("input_items").is_none());
        assert!(schema["properties"].get("input_chars").is_none());
        for (key, value) in [
            ("context_tokens", json!(0)),
            ("input_items", json!(50)),
            ("input_chars", json!(500)),
        ] {
            let mut invalid = json!({"provider_id":orch_types::new_id(),"model_name":"small"});
            invalid[key] = value;
            assert!(Config::read(&invalid).is_err());
        }
        Ok(())
    }

    #[test]
    fn each_answer_allowance_is_an_explicit_positive_choice_without_a_hidden_floor()
    -> Result<(), SummarizeError> {
        let base = json!({"provider_id":orch_types::new_id(),"model_name":"small"});
        for field in [
            "answer_max_output_tokens",
            "title_max_output_tokens",
            "goal_max_output_tokens",
        ] {
            let mut selected = base.clone();
            selected[field] = json!(0);
            assert!(Config::read(&selected).is_err());
            selected[field] = json!(50);
            let read = Config::read(&selected)?;
            assert_eq!(
                serde_json::to_value(&read)
                    .map_err(|error| SummarizeError::Config(error.to_string()))?[field],
                json!(50)
            );
        }
        Ok(())
    }
}

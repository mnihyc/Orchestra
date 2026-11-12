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

/// The ordinary short answer budget for titles and descriptions.
pub const OUTPUT_TOKENS: i64 = 120;
/// Ordinary input planning budget; an explicitly larger budget may use the model's available window.
pub const CONTEXT_TOKENS: u32 = 32_000;

const fn output_tokens() -> i64 {
    OUTPUT_TOKENS
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
    /// The maximum response token count.
    #[serde(default = "output_tokens")]
    pub max_output_tokens: i64,
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

    /// Reads the schema's configuration and refuses an empty name or nonpositive budget.
    pub fn read(value: &Json) -> Result<Self, SummarizeError> {
        let config: Self = serde_json::from_value(value.clone()).map_err(|_| {
            SummarizeError::Config("the selected model configuration does not fit".to_owned())
        })?;
        if config.model_name.trim().is_empty()
            || config.max_output_tokens <= 0
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
    for name in ["max_output_tokens", "context_tokens"] {
        schema["properties"][name]["minimum"] = serde_json::json!(1);
    }
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
    fn input_has_one_positive_token_default_and_output_is_unchanged() -> Result<(), SummarizeError>
    {
        let value = json!({"provider_id":orch_types::new_id(),"model_name":"small"});
        let config = Config::read(&value)?;
        assert_eq!(config.context_tokens, 32_000);
        assert_eq!(config.max_output_tokens, 120);
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
}

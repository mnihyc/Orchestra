//! Configuration schema and supported fields.
//!
//! The configuration is the rest of auto.summarizer after its kind, with bounded activity defaults.

use orch_summarizer_contract::descriptor::{SummarizerDescriptor, SummaryField};
use orch_summarizer_contract::summarizer::SummarizeError;
use orch_types::{Id, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The ordinary short answer budget for titles and descriptions.
pub const OUTPUT_TOKENS: i64 = 120;
/// The latest fifty items give a short status enough recent context.
pub const INPUT_ITEMS: u32 = 50;
/// A tool's title or a message preview is bounded to five hundred characters.
pub const INPUT_CHARS: u32 = 500;

const fn output_tokens() -> i64 {
    OUTPUT_TOKENS
}
const fn input_items() -> u32 {
    INPUT_ITEMS
}
const fn input_chars() -> u32 {
    INPUT_CHARS
}

/// Configuration of the built-in model producer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The API provider whose key pays.
    pub provider_id: Id,
    /// Its offered model's routing name.
    pub model_name: String,
    /// The maximum response token count.
    #[serde(default = "output_tokens")]
    pub max_output_tokens: i64,
    /// The most recent activity items to send.
    #[serde(default = "input_items")]
    pub input_items: u32,
    /// The maximum characters in each activity item.
    #[serde(default = "input_chars")]
    pub input_chars: u32,
    /// Replacement instructions by field; null keeps the built-in text.
    #[serde(default)]
    pub prompts: BTreeMap<String, Option<String>>,
}

impl Config {
    /// Reads the schema's configuration and refuses an empty name or nonpositive budget.
    pub fn read(value: &Json) -> Result<Self, SummarizeError> {
        let config: Self = serde_json::from_value(value.clone()).map_err(|_| {
            SummarizeError::Config("the selected model configuration does not fit".to_owned())
        })?;
        if config.model_name.trim().is_empty()
            || config.max_output_tokens <= 0
            || config.input_items == 0
            || config.input_chars == 0
        {
            return Err(SummarizeError::Config(
                "model name and output/activity budgets must be nonempty and positive".to_owned(),
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
    for name in ["max_output_tokens", "input_items", "input_chars"] {
        schema["properties"][name]["minimum"] = serde_json::json!(1);
    }
    schema["properties"]["model_name"]["pattern"] = serde_json::json!("\\S");
    SummarizerDescriptor {
        name: "model".to_owned(),
        title: "A model".to_owned(),
        config_schema: schema,
        fields: SummaryField::ALL.to_vec(),
        extensions: Default::default(),
    }
}

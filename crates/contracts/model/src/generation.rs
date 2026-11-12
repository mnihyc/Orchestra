//! Tool-capable generation without a credential, wire format or vendor in the host.

use crate::credentials::WireError;
use orch_types::{Hash, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Generation support is protocol-owned declaration data; the host never
/// identifies an interface by matching a vendor's name.
#[must_use]
pub fn supported(protocol: &str) -> bool {
    static DECLARATIONS: std::sync::OnceLock<Json> = std::sync::OnceLock::new();
    DECLARATIONS
        .get_or_init(|| {
            serde_json::from_str(include_str!("../../../../shared/protocol-options.json"))
                .unwrap_or(Json::Null)
        })
        .get("protocols")
        .and_then(|protocols| protocols.get(protocol))
        .and_then(|protocol| protocol.get("generation"))
        .and_then(|generation| generation.get("tools"))
        .and_then(Json::as_bool)
        .unwrap_or(false)
}

/// One tool exposed by the host. Its parameters use the API catalogue's JSON schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GenerationTool {
    /// Stable callable name.
    pub name: String,
    /// What this tool reads, with its scope and limitations.
    pub description: String,
    /// The schema of its arguments.
    pub parameters: Json,
}

/// Visible content and tool traffic in their original order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GenerationPart {
    /// Private image content fetched for this exact machine command.
    ImageRef {
        blob: Hash,
        size: i64,
        media_type: String,
        name: String,
        url: String,
    },
    /// Runtime-resolved image data. Never persisted in the control-plane command.
    Image {
        media_type: String,
        data: String,
        name: String,
    },
    /// User or assistant text.
    Text { text: String },
    /// An assistant's request, executed only by the control plane.
    ToolCall {
        id: String,
        name: String,
        arguments: Json,
    },
    /// One bounded, untrusted tool result.
    ToolResult {
        call_id: String,
        content: String,
        is_error: bool,
        /// Host-owned journal correlation; never serialized into the model's HTTP body or used as authority.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        witness: Option<orch_types::Id>,
    },
}

/// A dialogue author. Tool answers belong to the user's side of the exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GenerationRole {
    /// A person, including tool answers supplied by the host.
    User,
    /// The assistant, including its requested tools.
    Assistant,
}

/// A message retained between steps. Opaque continuation blocks are interpreted
/// only by their originating protocol extension (for example signed reasoning).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GenerationMessage {
    /// Person or assistant.
    pub role: GenerationRole,
    /// Text and tool parts.
    pub content: Vec<GenerationPart>,
    /// Protocol-owned continuation, never turned into a system instruction.
    #[serde(default)]
    pub continuation: Vec<Json>,
}

/// One generation step; the model is selected independently by the provider command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GenerationInput {
    /// The host's instruction, separate from all untrusted material.
    pub system: String,
    /// Current reauthorized conversation and tool results.
    pub messages: Vec<GenerationMessage>,
    /// The tools this step may request.
    pub tools: Vec<GenerationTool>,
    /// Positive output budget.
    pub max_output_tokens: i64,
}

/// Why the generation stopped. A cut stream is an error, not a completed answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GenerationStop {
    /// The assistant completed its answer.
    Complete,
    /// The host must supply the requested tools before continuing.
    Tools,
    /// The output budget ended before the answer completed.
    Limit,
    /// The upstream explicitly refused the request.
    Refused,
}

/// One completed model step. Usage is observed by the existing protocol tap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GenerationOutput {
    /// Passive native result envelope: `response` is the completed native body and
    /// `frames` keeps observed stream objects for extension-declared projection.
    /// This is untrusted output, never message continuation, executable tool input or authority.
    #[serde(default, skip_serializing_if = "Json::is_null")]
    pub native_result: Json,
    /// Text, tool calls and continuation for the next request.
    pub message: GenerationMessage,
    /// Completed, tools, limited or refused.
    pub stop: GenerationStop,
}

/// Incremental model content, before the complete step is committed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GenerationDelta {
    /// An ordered assistant text fragment.
    Text { text: String },
}

/// A protocol extension's stream reader. The runtime bounds total reply bytes
/// before feeding this reader and never executes an unfinished tool call.
pub trait GenerationStream: Send {
    /// Read another chunk and return newly visible text.
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<GenerationDelta>, WireError>;
    /// Require the protocol's terminal event and validate all completed tool arguments.
    fn finish(self: Box<Self>) -> Result<GenerationOutput, WireError>;
}

impl GenerationOutput {
    /// Validate the neutral result before the host considers any tool call.
    pub fn validate(&self) -> Result<(), WireError> {
        let bad = || WireError::Internal("the model returned invalid tool calls".to_owned());
        let mut ids = std::collections::HashSet::new();
        let mut calls = 0;
        if self.message.role != GenerationRole::Assistant {
            return Err(bad());
        }
        for part in &self.message.content {
            match part {
                GenerationPart::ToolCall {
                    id,
                    name,
                    arguments,
                } => {
                    calls += 1;
                    if id.is_empty() || name.is_empty() || !arguments.is_object() || !ids.insert(id)
                    {
                        return Err(bad());
                    }
                }
                GenerationPart::ToolResult { .. }
                | GenerationPart::ImageRef { .. }
                | GenerationPart::Image { .. } => return Err(bad()),
                GenerationPart::Text { .. } => {}
            }
        }
        if (self.stop == GenerationStop::Tools) != (calls > 0) {
            return Err(bad());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn passive_native_results_never_become_tool_calls_or_message_continuation() {
        let legacy = json!({"message":{"role":"assistant","content":[{"kind":"text","text":"Answer"}],"continuation":[]},"stop":"complete"});
        let parsed: GenerationOutput = serde_json::from_value(legacy.clone()).unwrap();
        assert!(parsed.native_result.is_null());
        assert_eq!(serde_json::to_value(parsed).unwrap(), legacy);
        let mut with_native = legacy;
        with_native["native_result"] = json!({"response":{"future_tool":{"name":"do_something","arguments":{}}},"frames":[{"unknown":"reported"}]});
        let parsed: GenerationOutput = serde_json::from_value(with_native).unwrap();
        assert!(parsed.validate().is_ok());
        assert!(parsed.message.continuation.is_empty());
        assert_eq!(parsed.message.content.len(), 1);
    }

    #[test]
    fn duplicate_or_unfinished_tool_calls_cannot_reach_the_host() {
        let call = GenerationPart::ToolCall {
            id: "call".into(),
            name: "read".into(),
            arguments: json!({}),
        };
        let mut output = GenerationOutput {
            native_result: serde_json::Value::Null,
            message: GenerationMessage {
                role: GenerationRole::Assistant,
                content: vec![call.clone()],
                continuation: vec![],
            },
            stop: GenerationStop::Tools,
        };
        assert!(output.validate().is_ok());
        output.message.content.push(call);
        assert!(output.validate().is_err());
        output.message.content.pop();
        output.stop = GenerationStop::Complete;
        assert!(output.validate().is_err());
    }
}

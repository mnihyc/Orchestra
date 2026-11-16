//! Provider-complete request and constrained output conversion.
//!
//! The host checks the selected writer's current authority and caps and counts every answered call.

use crate::{descriptor, prompts};
use async_trait::async_trait;
use orch_agent_contract::events::TokenUsage;
use orch_link_contract::commands::{ProviderComplete, ProviderCompleteResult};
use orch_summarizer_contract::descriptor::{SummarizerDescriptor, SummaryField};
use orch_summarizer_contract::result::{SummaryResult, UsageEvent};
use orch_summarizer_contract::summarizer::{SummarizeError, Summarizer, SummaryRequest};
use orch_types::Id;
use std::sync::Arc;

/// What a model's own parameters say about reasoning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reasoning {
    /// Whether output room is reserved for known or unrecorded upstream reasoning.
    pub reasons: bool,
    /// The lowest effort the model offers that the writer may use, to ask a summary for.
    pub lowest_effort: Option<String>,
}
impl Default for Reasoning {
    fn default() -> Self {
        Self {
            reasons: true,
            lowest_effort: None,
        }
    }
}

/// The host's permission-checked API-provider completion port.
#[async_trait]
pub trait Complete: Send + Sync {
    /// Calls the selected provider for work at this node without opening a harness.
    async fn complete(
        &self,
        node: Id,
        request: ProviderComplete,
    ) -> Result<ProviderCompleteResult, SummarizeError>;

    /// Reasoning room and allowed tuning for work at this node. A host with no model facts reserves
    /// the configured room and offers no effort control.
    async fn reasoning(
        &self,
        node: Id,
        provider_id: Id,
        model_name: &str,
    ) -> Result<Reasoning, SummarizeError> {
        let _ = (node, provider_id, model_name);
        Ok(Reasoning::default())
    }
}

/// The built-in model producer over a host's completion port.
pub struct Model {
    complete: Arc<dyn Complete>,
}

impl Model {
    /// Uses the host's canonical provider-complete path.
    #[must_use]
    pub fn new(complete: Arc<dyn Complete>) -> Self {
        Self { complete }
    }
}

#[async_trait]
impl Summarizer for Model {
    fn describe(&self) -> SummarizerDescriptor {
        descriptor::descriptor()
    }

    async fn summarize(
        &self,
        mut request: SummaryRequest,
    ) -> Result<SummaryResult, SummarizeError> {
        let config = descriptor::Config::read(&request.config)?;
        // Handoffs use their existing transcript/share planner; their previous text is not activity.
        if request.field != SummaryField::Handoff {
            let budget = crate::context::request_budget(&request, config.context_tokens)?;
            let total = u64::try_from(request.activity.len()).unwrap_or(u64::MAX);
            let input = std::mem::take(&mut request.activity)
                .into_iter()
                .enumerate()
                .map(|(index, item)| crate::context::InputItem {
                    key: index.to_string(),
                    item,
                })
                .collect();
            request.activity = crate::context::fill(input, budget, None, total)?.activity;
        }
        let (system, input) = prompts::render(&request, &config);
        let reasoning = self
            .complete
            .reasoning(request.node.id, config.provider_id, &config.model_name)
            .await?;
        let answer = self
            .complete
            .complete(
                request.node.id,
                config.completion_with(
                    &reasoning,
                    system,
                    input,
                    config.answer_tokens(request.field),
                ),
            )
            .await?;
        let tokens: TokenUsage = answer.usage;
        let usage = Some(UsageEvent {
            model: config.model_name,
            tokens,
            request_ref: answer.request_ref,
        });
        let result = if request.field == SummaryField::Tags {
            SummaryResult {
                text: None,
                tags: serde_json::from_str::<Vec<String>>(&answer.text).ok(),
                usage,
            }
        } else {
            SummaryResult {
                text: Some(answer.text),
                tags: None,
                usage,
            }
        };
        Ok(result
            .conform(request.field, &request.limits, &request.tag_choices)
            .result)
    }
}

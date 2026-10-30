//! Provider-complete request and constrained output conversion.
//!
//! The host checks the selected writer's current authority and caps and counts every answered call.

use crate::{descriptor, prompts};
use async_trait::async_trait;
use orch_agent_contract::events::TokenUsage;
use orch_link_contract::commands::{CompletionPurpose, ProviderComplete, ProviderCompleteResult};
use orch_summarizer_contract::descriptor::{SummarizerDescriptor, SummaryField};
use orch_summarizer_contract::result::{SummaryResult, UsageEvent};
use orch_summarizer_contract::summarizer::{SummarizeError, Summarizer, SummaryRequest};
use orch_types::Id;
use std::sync::Arc;

/// The host's permission-checked API-provider completion port.
#[async_trait]
pub trait Complete: Send + Sync {
    /// Calls the selected provider for work at this node without opening a harness.
    async fn complete(
        &self,
        node: Id,
        request: ProviderComplete,
    ) -> Result<ProviderCompleteResult, SummarizeError>;
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

    async fn summarize(&self, request: SummaryRequest) -> Result<SummaryResult, SummarizeError> {
        let config = descriptor::Config::read(&request.config)?;
        let (system, input) = prompts::render(&request, &config);
        let answer = self
            .complete
            .complete(
                request.node.id,
                ProviderComplete {
                    params: None,
                    provider_id: config.provider_id,
                    purpose: CompletionPurpose::Summary,
                    // The host replaces this with a protocol this actual provider offers.
                    protocol: String::new(),
                    model_name: config.model_name.clone(),
                    system,
                    input,
                    max_output_tokens: config.max_output_tokens,
                },
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

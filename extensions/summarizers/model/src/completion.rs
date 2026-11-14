//! Provider-complete request and constrained output conversion.
//!
//! The host checks the selected writer's current authority and caps and counts every answered call.

use crate::{descriptor, prompts};
use async_trait::async_trait;
use orch_agent_contract::events::TokenUsage;
use orch_link_contract::commands::{CompletionPurpose, ProviderComplete, ProviderCompleteResult};
use orch_provider_contract::capabilities::BackendRef;
use orch_summarizer_contract::descriptor::{SummarizerDescriptor, SummaryField};
use orch_summarizer_contract::result::{SummaryResult, UsageEvent};
use orch_summarizer_contract::summarizer::{SummarizeError, Summarizer, SummaryRequest};
use orch_types::Id;
use orch_types::Json;
use std::sync::Arc;

/// A selected backend and its rendered material, before a native transport has been chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionRequest {
    /// Exact route, account offering and adapter references; never acting authority.
    pub backend: BackendRef,
    /// Existing summary accounting purpose.
    pub purpose: CompletionPurpose,
    /// Explicit model options; current defaults are resolved by the bound host.
    pub params: Option<Json>,
    /// The producer's field-specific instructions.
    pub system: String,
    /// The checked material supplied by the producer.
    pub input: String,
    /// The configured answer allowance.
    pub max_output_tokens: i64,
}

impl CompletionRequest {
    /// Only the installed API route becomes an API command; a native route cannot change its bill.
    pub fn into_api(self) -> Result<ProviderComplete, SummarizeError> {
        if self.backend.route != crate::capabilities::ROUTE {
            return Err(SummarizeError::Skipped(
                "The selected backend does not use the API completion port.".into(),
            ));
        }
        self.backend
            .validate()
            .map_err(|reason| SummarizeError::Config(reason.into()))?;
        if !(self.backend.config.is_null()
            || self
                .backend
                .config
                .as_object()
                .is_some_and(serde_json::Map::is_empty))
        {
            return Err(SummarizeError::Config(
                "The API summary route has no adapter placement options.".into(),
            ));
        }
        let offering = self.backend.offering.ok_or_else(|| {
            SummarizeError::Config("The summary backend needs an account offering.".into())
        })?;
        Ok(ProviderComplete {
            model_parameters: None,
            provider_id: offering.provider,
            purpose: self.purpose,
            protocol: String::new(),
            model_name: offering.offering,
            params: self.params,
            system: self.system,
            input: self.input,
            max_output_tokens: self.max_output_tokens,
        })
    }
}

/// An answer from the selected transport; absent measurements remain unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionAnswer {
    /// Actual returned text before field conformance.
    pub text: String,
    /// Reported use only; the host owns the authoritative ledger.
    pub usage: Option<TokenUsage>,
    /// Stable actual request or native receipt reference.
    pub request_ref: Option<String>,
}

impl From<ProviderCompleteResult> for CompletionAnswer {
    fn from(answer: ProviderCompleteResult) -> Self {
        Self {
            text: answer.text,
            usage: Some(answer.usage),
            request_ref: answer.request_ref,
        }
    }
}

/// The host's source-bound completion port; backend facts confer no source authority.
#[async_trait]
pub trait Complete: Send + Sync {
    /// Executes only through this producer's actual current authority and selected backend.
    async fn complete(
        &self,
        node: Id,
        request: CompletionRequest,
    ) -> Result<CompletionAnswer, SummarizeError>;
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
        let answer = self
            .complete
            .complete(
                request.node.id,
                config.completion(system, input, config.max_output_tokens),
            )
            .await?;
        let model = config.offering()?.offering.clone();
        let usage = answer.usage.map(|tokens| UsageEvent {
            model,
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

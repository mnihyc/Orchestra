//! Summary capability over the existing completion host's admission, recovery and ledger path.
use crate::completion::{Complete, Model};
use crate::descriptor::Config;
use async_trait::async_trait;
use orch_provider_contract::capabilities::{
    CallSource, CapabilityCall, CapabilityExecutor, CapabilityItem, CapabilityResult, Declarations,
    Route,
};
use orch_provider_contract::{SourceError, SourceErrorCode};
use orch_summarizer_contract::descriptor::SummaryField;
use orch_summarizer_contract::summarizer::{
    OUTPUT_CUT, SummarizeError, Summarizer, SummaryRequest,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Namespaced route, additive to the common summary capability.
pub const ROUTE: &str = "summarizer-model.complete";
/// The actual registered adapter; declarations alone never install it.
pub const EXECUTOR: &str = "summarizer-model";

/// Model-backed summaries use a generation offering directly, with no program/native-tool requirement.
#[must_use]
pub fn declarations() -> Declarations {
    let mut declarations = Declarations::default();
    declarations.routes.insert(ROUTE.into(), Route {
        title: "Model summarizer".into(),
        text: BTreeMap::from([("en".into(), "Model summarizer".into()), ("zh-CN".into(), "模型摘要器".into())]),
        kind: "service".into(), executor: Some(EXECUTOR.into()), capabilities: vec!["summary".into()],
        uses_backend: true, requires_opt_in: true, requires_offering: true, requires_interface: true,
        eligible_when: json!({"required":["offering"],"properties":{"offering":{"required":["kind","capabilities"],"properties":{
            "kind":{"const":"model"},"capabilities":{"required":["generation"],"properties":{"generation":{"required":["enabled"],"properties":{"enabled":{"const":true}}}}}
        }}}}),
        ..Route::default()
    });
    declarations.route_links.insert(
        "summary".into(),
        BTreeMap::from([("service".into(), vec![ROUTE.into()])]),
    );
    declarations
}

/// The trusted completion host retains actual current-source/credential/effect/usage responsibilities.
pub struct Executor {
    host: Arc<dyn Complete>,
    source: CallSource,
    field: SummaryField,
}

impl Executor {
    /// Binds the same logical source as the host; no caller or credential is rebuilt from these ids.
    #[must_use]
    pub fn new(host: Arc<dyn Complete>, source: CallSource) -> Self {
        Self {
            host,
            source,
            field: SummaryField::Summary,
        }
    }
    /// The native title workflow owns its separate source/cache and binds this field explicitly.
    #[must_use]
    pub fn title(host: Arc<dyn Complete>, source: CallSource) -> Self {
        Self {
            host,
            source,
            field: SummaryField::Title,
        }
    }
}

fn malformed() -> SourceError {
    SourceError::new(
        SourceErrorCode::Malformed,
        "the summary call does not match its admitted source or model",
    )
}

fn failure(error: SummarizeError) -> SourceError {
    let (code, retryable, message) = match error {
        SummarizeError::Config(_) => (
            SourceErrorCode::Malformed,
            false,
            "the summary model configuration is not usable",
        ),
        SummarizeError::Unsupported(_) => (
            SourceErrorCode::Unsupported,
            false,
            "the producer does not offer a summary paragraph",
        ),
        SummarizeError::Skipped(_) => (
            SourceErrorCode::Refused,
            false,
            "the summary completion was refused",
        ),
        // The answer ran into its output allowance: its own code, so that whoever is told can raise it.
        SummarizeError::Failed { message, retryable } if message == OUTPUT_CUT => {
            (SourceErrorCode::TooLarge, retryable, OUTPUT_CUT)
        }
        SummarizeError::Failed { retryable, .. } => (
            SourceErrorCode::Unavailable,
            retryable,
            "the model could not produce a summary",
        ),
        SummarizeError::Internal(_) => (
            SourceErrorCode::Unavailable,
            true,
            "the summary could not be completed",
        ),
    };
    SourceError {
        code,
        retryable,
        message: message.into(),
    }
}

#[async_trait]
impl CapabilityExecutor for Executor {
    async fn execute(&self, call: CapabilityCall) -> Result<CapabilityResult, SourceError> {
        if call.source != self.source
            || call.capability != "summary"
            || call.item != "summary"
            || call.ledger_unit != "token"
            || call.route.id != ROUTE
            || call.route.executor.as_deref() != Some(EXECUTOR)
        {
            return Err(malformed());
        }
        let request: SummaryRequest =
            serde_json::from_value(call.input).map_err(|_| malformed())?;
        if request.field != self.field
            || request.limits.max_chars == 0
            || self.source.node_id.or(self.source.thread_id) != Some(request.node.id)
        {
            return Err(malformed());
        }
        let config = Config::read(&request.config).map_err(failure)?;
        if !call.route.offering.as_ref().is_some_and(|offering| {
            offering.provider == config.provider_id && offering.offering == config.model_name
        }) {
            return Err(malformed());
        }
        let result = Model::new(self.host.clone())
            .summarize(request)
            .await
            .map_err(failure)?;
        let text = result
            .text
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| {
                SourceError::new(
                    SourceErrorCode::Malformed,
                    "the model returned no usable summary",
                )
            })?;
        let native_ref = result.usage.and_then(|usage| usage.request_ref);
        // The completion host already records normalized usage/timing. This narrower producer result has
        // no measurement-state envelope, so it must not turn a missing report into measured zero tokens.
        Ok(CapabilityResult {
            items: vec![CapabilityItem {
                family: "summary".into(),
                native_ref,
                payload: json!({"text":text}),
            }],
            usage: orch_types::Json::Null,
        })
    }
}

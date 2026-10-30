//! External Summarizer bridge.
//!
//! [`SummarizerBridge`] is a [`Summarizer`] whose implementation is a program. A summarizer turns the activity of
//! a node into the text of one of its fields; the engine owns when and how often, and the rule that a person's
//! edit wins, so the program only has to answer one question.
//!
//! | Method | Required | Parameters | Result |
//! |--------|----------|------------|--------|
//! | `describe` | yes | none | the summarizer's descriptor |
//! | `summarize` | yes | the request: `field`, `node`, `previous`, `activity`, `tag_choices`, `limits`, `config` | the result: `text`, `tags` and `usage`, each of which may be `null` |
//!
//! A request for a field that the descriptor does not list is refused without asking the program. A program
//! answers with the error code `skipped` for a summary that is not made because it may not be (not a failure),
//! `config` for a configuration it cannot use, `unsupported` for a field it does not write; any other code is a
//! failure, and `retryable` says whether the next attempt may succeed.

use async_trait::async_trait;
use orch_plugin_contract::PluginKind;
use orch_summarizer_contract::descriptor::{SummarizerDescriptor, SummaryField};
use orch_summarizer_contract::result::SummaryResult;
use orch_summarizer_contract::summarizer::{SummarizeError, Summarizer, SummaryRequest};

use super::{BridgeError, Described, params, result};
use crate::requests::CallError;
use crate::supervisor::Plugin;

/// The method that makes a summary.
const SUMMARIZE: &str = "summarize";

/// A summarizer that is a program.
pub struct SummarizerBridge {
    plugin: Plugin,
    descriptor: Described<SummarizerDescriptor>,
}

impl SummarizerBridge {
    /// The summarizer that `plugin` is. The plugin must be of the kind and have said hello.
    pub fn new(plugin: Plugin) -> Result<Self, BridgeError> {
        Ok(Self {
            descriptor: Described::new(plugin.clone(), PluginKind::Summarizer)?,
            plugin,
        })
    }
}

fn failed(error: &CallError) -> SummarizeError {
    SummarizeError::Failed {
        message: error.to_string(),
        retryable: error.retryable(),
    }
}

/// The error of a summary for what the program answered.
fn error_of(error: CallError, field: SummaryField) -> SummarizeError {
    match &error {
        CallError::Failed(remote) => match remote.code.as_str() {
            "unsupported" => SummarizeError::Unsupported(field),
            "config" => SummarizeError::Config(remote.message.clone()),
            "skipped" => SummarizeError::Skipped(remote.message.clone()),
            "internal" => SummarizeError::Internal(remote.message.clone()),
            _ => SummarizeError::Failed {
                message: remote.message.clone(),
                retryable: remote.retryable,
            },
        },
        CallError::Malformed { .. } => SummarizeError::Internal(error.to_string()),
        _ => failed(&error),
    }
}

#[async_trait]
impl Summarizer for SummarizerBridge {
    fn describe(&self) -> SummarizerDescriptor {
        (*self.descriptor.get()).clone()
    }

    async fn summarize(&self, request: SummaryRequest) -> Result<SummaryResult, SummarizeError> {
        let field = request.field;
        if !self.descriptor.get().writes(field) {
            return Err(SummarizeError::Unsupported(field));
        }
        crate::schema::validate(&self.descriptor.get().config_schema, &request.config)
            .map_err(|error| SummarizeError::Config(error.to_string()))?;
        let answer = async {
            let sent = params(SUMMARIZE, &request)?;
            result(SUMMARIZE, self.plugin.call(SUMMARIZE, sent).await?)
        };
        answer.await.map_err(|error| error_of(error, field))
    }
}

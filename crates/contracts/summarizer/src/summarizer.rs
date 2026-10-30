//! Summarize operation and typed failure boundary.
//!
//! A **summarizer** turns activity into text. Each of a node's title, description and tags is maintained by
//! people or by automation; what automation does is configuration (`auto.title`, `auto.description`,
//! `auto.tags`), and `auto.summarizer` names the implementation and configures it. The engine owns
//! triggering, rate limiting, the rule that a person's edit wins, and the ledger; a summarizer only turns
//! activity into text. A summarizer that needs no model (rules, templates, an internal service) is as valid
//! as one that calls one.
//!
//! The built-in kind `model` asks a small model through a provider. A failure of a summary adds nothing to
//! any timeline: it is shown on the setting's row and counted in metrics, and the next attempt backs off,
//! doubling from `min_interval_secs` up to one hour ([`next_interval`]). A summary that is skipped because the
//! provider or the model may not be used there is no failure, and does not back off.

use std::time::Duration;

use async_trait::async_trait;
use orch_plugin_contract::refs::NodeRef;
use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::activity::ActivityItem;
use crate::descriptor::{SummarizerDescriptor, SummaryField};
use crate::result::SummaryResult;

/// The longest title: 80 characters.
pub const TITLE_MAX_CHARS: u32 = 80;
/// The longest description: one line of 200 characters.
pub const DESCRIPTION_MAX_CHARS: u32 = 200;
/// A handoff stays small without truncating an exact path or command.
pub const HANDOFF_MAX_CHARS: u32 = 3_000;
/// The handoff producer's maximum word count.
pub const HANDOFF_MAX_WORDS: usize = 400;
/// The longest the pause after failures grows to: one hour.
pub const BACKOFF_CEILING: Duration = Duration::from_secs(60 * 60);

/// How much a summary may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SummaryLimits {
    /// The most characters of a title or a description.
    pub max_chars: u32,
    /// The most tags (`tags.max_per_place`).
    pub max_tags: u32,
}

impl SummaryLimits {
    /// The limits for writing `field`, with `max_tags` tags at most.
    #[must_use]
    pub const fn for_field(field: SummaryField, max_tags: u32) -> Self {
        Self {
            max_chars: match field {
                SummaryField::Title => TITLE_MAX_CHARS,
                SummaryField::Description | SummaryField::Rollup => DESCRIPTION_MAX_CHARS,
                SummaryField::Tags => 0,
                SummaryField::Handoff => HANDOFF_MAX_CHARS,
            },
            max_tags,
        }
    }
}

/// What a summarizer is asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SummaryRequest {
    /// The field to write.
    pub field: SummaryField,
    /// The node it is for.
    pub node: NodeRef,
    /// The field's value now; for tags, the names joined by commas.
    pub previous: String,
    /// What happened since the last summary: redacted, and cut.
    pub activity: Vec<ActivityItem>,
    /// For tags: the names it may choose among, those defined for the node's project and org.
    #[serde(default)]
    pub tag_choices: Vec<String>,
    /// How much the answer may be.
    pub limits: SummaryLimits,
    /// The rest of `auto.summarizer`, validated against the summarizer's `config_schema`.
    #[serde(default)]
    pub config: Json,
}

/// Why a summary was not made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SummarizeError {
    /// The summarizer does not write that field.
    #[error("this summarizer does not write the field {0:?}")]
    Unsupported(SummaryField),
    /// The configuration does not fit what the summarizer needs, though it passed the schema.
    #[error("the configuration is not usable: {0}")]
    Config(String),
    /// The summary is skipped: the provider or the model may not be used there, or a cap has no room. It is
    /// shown on the setting's row, and is no failure.
    #[error("skipped: {0}")]
    Skipped(String),
    /// What the summarizer called failed.
    #[error("the call failed: {message}")]
    Failed {
        /// What happened.
        message: String,
        /// Whether trying again may help.
        retryable: bool,
    },
    /// The summarizer failed in a way that is its own.
    #[error("internal error: {0}")]
    Internal(String),
}

impl SummarizeError {
    /// Whether it counts as a failure: it is counted in metrics, shown on the setting's row, and the next
    /// attempt backs off. A skip is not one.
    #[must_use]
    pub const fn is_failure(&self) -> bool {
        !matches!(self, Self::Skipped(_))
    }

    /// The code, for metrics and for the setting's row.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Unsupported(_) => "unsupported",
            Self::Config(_) => "config",
            Self::Skipped(_) => "skipped",
            Self::Failed { .. } => "failed",
            Self::Internal(_) => "internal",
        }
    }
}

/// How long to wait before the next attempt after `failures` failures in a row: `min_interval` doubled for
/// each, and never more than [`BACKOFF_CEILING`]. With none, the minimum interval itself.
#[must_use]
pub fn next_interval(min_interval: Duration, failures: u32) -> Duration {
    let factor = 1u32.checked_shl(failures.min(31)).unwrap_or(u32::MAX);
    min_interval.saturating_mul(factor).min(BACKOFF_CEILING)
}

/// One implementation per kind, used by `orch-api`.
#[async_trait]
pub trait Summarizer: Send + Sync + 'static {
    /// Its name, its title, the schema of its configuration, and which fields it can write.
    fn describe(&self) -> SummarizerDescriptor;

    /// Turns activity into the text of one field.
    async fn summarize(&self, request: SummaryRequest) -> Result<SummaryResult, SummarizeError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use serde_json::json;

    /// A summarizer with no model: it writes a description from the last thing said.
    struct Rules;

    #[async_trait]
    impl Summarizer for Rules {
        fn describe(&self) -> SummarizerDescriptor {
            serde_json::from_value(json!({ "name": "rules", "title": "By rules", "config_schema": {}, "fields": ["description"] })).unwrap()
        }
        async fn summarize(
            &self,
            request: SummaryRequest,
        ) -> Result<SummaryResult, SummarizeError> {
            if request.field != SummaryField::Description {
                return Err(SummarizeError::Unsupported(request.field));
            }
            let latest = request
                .activity
                .last()
                .ok_or_else(|| SummarizeError::Skipped("nothing happened".into()))?;
            Ok(SummaryResult::text(&format!("Latest: {}", latest.text)))
        }
    }

    fn node() -> NodeRef {
        serde_json::from_value(json!({ "id": "0199abcd-1234-7000-8000-0000000000a1", "kind": "thread", "name": "Fix login", "redacted": false })).unwrap()
    }

    fn request(field: SummaryField, activity: &[&str]) -> SummaryRequest {
        SummaryRequest {
            field,
            node: node(),
            previous: String::new(),
            activity: activity
                .iter()
                .map(|text| ActivityItem {
                    kind: crate::activity::ActivityKind::AgentMessage,
                    text: (*text).into(),
                    at: None,
                })
                .collect(),
            tag_choices: vec![],
            limits: SummaryLimits::for_field(field, 3),
            config: json!({}),
        }
    }

    #[test]
    fn a_summarizer_with_no_model_writes_a_description_and_says_which_fields_it_does_not() {
        let rules = Rules;
        let described = rules.describe();
        assert_eq!(described.check(), Ok(()));
        let result = block_on(rules.summarize(request(
            SummaryField::Description,
            &["read the handler", "fixed the redirect"],
        )))
        .unwrap();
        assert_eq!(result.text.as_deref(), Some("Latest: fixed the redirect"));
        assert_eq!(
            result.usage, None,
            "a summarizer that calls no model reports no usage"
        );
        assert_eq!(
            block_on(rules.summarize(request(SummaryField::Title, &["x"]))),
            Err(SummarizeError::Unsupported(SummaryField::Title))
        );
    }

    #[test]
    fn a_skip_is_no_failure_and_everything_else_is() {
        let skipped =
            block_on(Rules.summarize(request(SummaryField::Description, &[]))).unwrap_err();
        assert!(!skipped.is_failure());
        assert_eq!(skipped.code(), "skipped");
        for failure in [
            SummarizeError::Unsupported(SummaryField::Tags),
            SummarizeError::Config("no model".into()),
            SummarizeError::Failed {
                message: "503".into(),
                retryable: true,
            },
            SummarizeError::Internal("bug".into()),
        ] {
            assert!(failure.is_failure(), "{failure}");
        }
        assert_eq!(
            SummarizeError::Failed {
                message: "x".into(),
                retryable: false
            }
            .code(),
            "failed"
        );
    }

    #[test]
    fn after_failures_the_next_attempt_backs_off_doubling_up_to_one_hour() {
        let min = Duration::from_secs(60);
        let waits: Vec<u64> = (0..9)
            .map(|failures| next_interval(min, failures).as_secs())
            .collect();
        assert_eq!(waits, [60, 120, 240, 480, 960, 1920, 3600, 3600, 3600]);
        assert_eq!(next_interval(min, u32::MAX), BACKOFF_CEILING);
        assert_eq!(
            next_interval(Duration::from_secs(7200), 0),
            BACKOFF_CEILING,
            "a minimum above the ceiling is held to it"
        );
        assert_eq!(next_interval(Duration::ZERO, 5), Duration::ZERO);
    }

    #[test]
    fn a_request_reads_as_the_spec_writes_it_and_carries_the_limits_of_its_field() {
        let written = request(SummaryField::Title, &["fix the login redirect"]);
        let back: SummaryRequest =
            serde_json::from_value(serde_json::to_value(&written).unwrap()).unwrap();
        assert_eq!(back, written);
        assert_eq!(
            written.limits,
            SummaryLimits {
                max_chars: 80,
                max_tags: 3
            }
        );
        assert_eq!(
            SummaryLimits::for_field(SummaryField::Description, 5).max_chars,
            200
        );
    }
}

//! Neutral per-request observations shared by built-in, external and future command-line adapters.

use crate::events::turn_error;
/// Nullable counts shared with the public request ledger.
pub use orch_api_contract::usage::RequestTokens;
use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The plugin event name; its body is a ModelRequestReport on every adapter transport.
pub const MODEL_REQUEST_EVENT: &str = "model.request";

fn is_main(subagent: &bool) -> bool {
    !*subagent
}

/// A completed request, with a normalized error and the vendor's own words where supplied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RequestOutcome {
    /// The request completed.
    Done,
    /// The request failed or was cancelled.
    Error {
        /// An existing normalized turn error code.
        code: String,
        /// A safe explanation in plain words.
        message: String,
        /// The vendor's original words, where supplied.
        said: Option<String>,
    },
}

/// Whether a failure code belongs to the existing neutral turn-error vocabulary.
pub fn normalized_code(code: &str) -> bool {
    [
        turn_error::PROVIDER_EXHAUSTED,
        turn_error::PROVIDER_UNAUTHORIZED,
        turn_error::PROVIDER_BUSY,
        turn_error::PROVIDER_UNREACHABLE,
        turn_error::MODEL_UNAVAILABLE,
        turn_error::MODEL_REFUSED,
        turn_error::CONTEXT_OVERFLOW,
        turn_error::ENVIRONMENT_UNREACHABLE,
        turn_error::HARNESS_ERROR,
        turn_error::SESSION_LOST,
        turn_error::FORCED,
        turn_error::START_TIMEOUT,
        turn_error::CAP_REACHED,
    ]
    .contains(&code)
}

impl RequestOutcome {
    /// The ledger's outcome code.
    pub fn code(&self) -> &str {
        match self {
            Self::Done => "done",
            Self::Error { code, .. } => code,
        }
    }
}

/// One request's observations; repeated snapshots reuse the id, and only its final report carries counts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct UsageRequest {
    /// Stable within the turn and reused by every observation of this request.
    pub id: String,
    /// The model actually used, where reported.
    pub model: Option<String>,
    /// A child model call made while a tool/sub-agent runs; defaults to the main stream.
    #[serde(skip_serializing_if = "is_main")]
    pub subagent: bool,
    /// Input or a tool result given back; null means not measured.
    pub asked: Option<Time>,
    /// First output event; null means pending or not measured.
    pub first_output: Option<Time>,
    /// Last output boundary; null means running or not measured.
    pub end: Option<Time>,
    /// Counts known at the end; each unknown count is null, never an invented zero.
    pub tokens: RequestTokens,
    /// Thinking was not streamed; its time is inside time to first output.
    pub reasoning_hidden: bool,
    /// Whether token-level output was streamed, where observed.
    pub output_streamed: Option<bool>,
    /// Null until completion, then done or a normalized error.
    pub outcome: Option<RequestOutcome>,
}

impl UsageRequest {
    /// Sets a terminal outcome without calculating any metric.
    pub fn finish(&mut self, code: &str, message: Option<&str>, said: Option<&str>) {
        self.outcome = Some(if code == "done" {
            RequestOutcome::Done
        } else {
            RequestOutcome::Error {
                code: if normalized_code(code) {
                    code
                } else {
                    turn_error::HARNESS_ERROR
                }
                .to_owned(),
                message: message
                    .unwrap_or("The model request did not complete.")
                    .to_owned(),
                said: said.map(str::to_owned),
            }
        });
    }

    /// Computes figures through the one shared arithmetic module, independent of provider or adapter.
    pub fn measured(&self) -> orch_api_contract::usage::RequestTiming {
        use orch_api_contract::usage::{RequestSample, UsageMeasurement};
        let (message, said) = match self.outcome.as_ref() {
            Some(RequestOutcome::Error { message, said, .. }) => {
                (Some(message.clone()), said.clone())
            }
            _ => (None, None),
        };
        orch_api_contract::performance::request_timing(&RequestSample {
            asked: self.asked,
            first_output: self.first_output,
            end: self.end,
            output_tokens: self.tokens.output_tokens,
            reasoning_tokens: self.tokens.reasoning_tokens,
            subagent: self.subagent,
            reasoning_hidden: self.reasoning_hidden,
            output_streamed: self.output_streamed,
            reported_tokens: Some(self.tokens.clone()),
            vendor_words: said,
            error_message: message,
            outcome: self
                .outcome
                .as_ref()
                .map_or(UsageMeasurement::Unread, |outcome| UsageMeasurement::Told {
                    value: outcome.code().to_owned(),
                }),
        })
    }

    /// Checks observable ordering and known counts; absence is permitted and is never replaced by zero.
    pub fn check(&self) -> Result<(), String> {
        if self.id.is_empty() {
            return Err("a model request needs its stable id".into());
        }
        for (a, b) in [
            (self.asked, self.first_output),
            (self.first_output, self.end),
            (self.asked, self.end),
        ] {
            if a.zip(b).is_some_and(|(a, b)| b < a) {
                return Err("model request times are out of order".into());
            }
        }
        if [self.asked, self.first_output, self.end]
            .into_iter()
            .flatten()
            .any(|at| at.unix_millis() == 0)
        {
            return Err("use null for an unmeasured timestamp, never the epoch".into());
        }
        if self.first_output.is_some() && self.tokens.output_tokens == Some(0) {
            return Err("a first-output event cannot accompany zero output tokens".into());
        }
        if self.output_streamed == Some(true) && self.first_output.is_none() {
            return Err("streamed output needs an observed first output".into());
        }
        let counts = [
            self.tokens.input_tokens,
            self.tokens.cached_input_tokens,
            self.tokens.cache_write_tokens,
            self.tokens.output_tokens,
            self.tokens.reasoning_tokens,
            self.tokens.total_tokens,
        ];
        if counts.iter().flatten().any(|n| *n < 0) {
            return Err("model request counts cannot be negative".into());
        }
        if self
            .tokens
            .reasoning_tokens
            .zip(self.tokens.output_tokens)
            .is_some_and(|(part, whole)| part > whole)
        {
            return Err("reasoning is a part of output".into());
        }
        if self
            .tokens
            .cached_input_tokens
            .zip(self.tokens.input_tokens)
            .is_some_and(|(part, whole)| part > whole)
            || self
                .tokens
                .cache_write_tokens
                .zip(self.tokens.input_tokens)
                .is_some_and(|(part, whole)| part > whole)
        {
            return Err("cached counts are parts of input".into());
        }
        if self.outcome.is_none() && counts.iter().any(Option::is_some) {
            return Err("model request counts belong to its terminal report".into());
        }
        if let Some(RequestOutcome::Error { code, .. }) = &self.outcome
            && !normalized_code(code)
        {
            return Err("the model request has no normalized error code".into());
        }
        Ok(())
    }
}

/// The first-class adapter event, routed identically for a built-in adapter and an external JSON line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelRequestReport {
    /// The thread.
    pub thread_id: Id,
    /// The session segment.
    pub segment_id: Id,
    /// The turn.
    pub turn_id: Id,
    /// The raw observations; adapters do not send derived durations or speeds.
    pub request: UsageRequest,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::AdapterEvent;
    use serde_json::json;

    #[test]
    fn an_external_adapter_and_a_builtin_report_the_same_request_with_nullable_counts() {
        let body = json!({"thread_id":"00000000-0000-4000-8000-000000000001","segment_id":"00000000-0000-4000-8000-000000000002","turn_id":"00000000-0000-4000-8000-000000000003",
            "request":{"id":"request-1","model":"model-a","asked":"2026-10-15T00:00:00.000Z","first_output":"2026-10-15T00:00:01.000Z","end":"2026-10-15T00:00:03.000Z",
            "tokens":{"input_tokens":100,"cached_input_tokens":0,"cache_write_tokens":null,"output_tokens":64,"reasoning_tokens":null,"total_tokens":164},"reasoning_hidden":false,"output_streamed":true,"outcome":{"status":"done"}}});
        let event = AdapterEvent::from_wire(MODEL_REQUEST_EVENT, body.clone()).unwrap();
        let (name, written) = event.to_wire().unwrap();
        assert_eq!(name, MODEL_REQUEST_EVENT);
        assert_eq!(written, body);
        let AdapterEvent::ModelRequest(report) = event else {
            panic!("a first-class model-request event");
        };
        report.request.check().unwrap();
        assert_eq!(report.request.measured().speed_milli.value(), Some(&32_000));
        assert_eq!(report.request.tokens.cache_write_tokens, None);
        assert_eq!(report.request.tokens.cached_input_tokens, Some(0));
    }

    #[test]
    fn missing_observations_are_not_a_zero_and_invalid_order_is_refused() {
        let mut request = UsageRequest {
            id: "unknown".into(),
            ..Default::default()
        };
        request.finish("done", None, None);
        assert_eq!(request.measured().speed_milli.value(), None);
        assert_eq!(request.measured().first_token_ms.value(), None);
        request.check().unwrap();
        request.asked = Some("2026-10-15T00:00:02Z".parse().unwrap());
        request.first_output = Some("2026-10-15T00:00:01Z".parse().unwrap());
        assert!(request.check().is_err());
    }
}

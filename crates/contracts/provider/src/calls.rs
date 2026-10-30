//! Inherited recovery rules, ordered steps and restartable progress for every model-call purpose.

use orch_types::{Id, Json, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The purpose matched by recovery rules; all purposes share their subject's repeat guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    /// An authored agent turn.
    Agent,
    /// A summary, including handoff summaries.
    Summary,
    /// An embedding request.
    Embedding,
    /// A permission or other configured model review.
    Review,
}

impl Purpose {
    /// The stored and wire name of the purpose.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Summary => "summary",
            Self::Embedding => "embedding",
            Self::Review => "review",
        }
    }
}

/// A configured failover destination; no candidate is ever discovered automatically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    /// The provider whose access is checked when the step acts.
    pub provider_id: Id,
    /// The model's routing name.
    pub model: String,
    /// An alternative agent profile where a different harness is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<Id>,
    /// Model and harness parameters, including any access programme.
    #[serde(default = "empty_params")]
    pub params: Json,
    /// Embedding dimensions, required to match the active generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<u32>,
}

fn empty_params() -> Json {
    serde_json::json!({})
}

/// Conditions are combined, with absent members leaving that part unrestricted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct When {
    /// A failure kind, or any failure when absent or `any`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// A refusal or missing-access reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// A provider, where the rule applies only to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<Id>,
    /// A model, where the rule applies only to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// A call purpose, where the rule applies only to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<Purpose>,
    /// An upstream HTTP status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<i64>,
    /// Case-insensitive words contained in the original vendor message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<String>,
    /// A case-insensitive regular expression matched against the vendor message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
}

/// What a wait needs before work may resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Until {
    /// A person chooses the next act.
    Person,
    /// The vendor's known limit reset; without one a person is needed.
    Reset,
}

/// A rule's ordered recovery steps; exhausting one advances to the next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    /// Repeat safely, continuing an agent turn if it already had effects.
    Retry {
        /// The pause when no vendor time was given.
        delay_secs: i64,
        /// How many attempts this step permits.
        times: u32,
        /// The multiplier of the fallback pause after each attempt.
        backoff: u32,
    },
    /// Continue an agent turn with effects, or rerun one without effects.
    Continue {
        /// How many attempts this step permits.
        times: u32,
    },
    /// Try the configured destination if the current person may use it.
    Failover {
        /// The only destination this step may select.
        candidate: Candidate,
    },
    /// Stop automatic handling until the named condition.
    Wait {
        /// A person or a known reset.
        until: Until,
    },
}

/// The first matching rule owns the failure until its step chain is exhausted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// A stable identifier retained on automatic inputs and timeline events.
    pub id: String,
    /// The matching conditions.
    pub when: When,
    /// The ordered recovery steps.
    pub steps: Vec<Step>,
}

/// Facts supplied to a rule without reading or mutating any storage.
pub struct Match<'a> {
    /// The neutral failure kind.
    pub kind: &'a str,
    /// A refusal or access reason.
    pub reason: Option<&'a str>,
    /// The actual provider that served the call.
    pub provider: Id,
    /// The actual model that served the call.
    pub model: &'a str,
    /// The call's purpose.
    pub purpose: Purpose,
    /// The original status.
    pub status: Option<i64>,
    /// The original, already redacted vendor words.
    pub words: &'a str,
}

impl When {
    /// Whether all supplied conditions match this failure.
    #[must_use]
    pub fn matches(&self, failure: &Match<'_>) -> bool {
        self.kind
            .as_deref()
            .is_none_or(|kind| kind == "any" || kind == failure.kind)
            && self
                .reason
                .as_deref()
                .is_none_or(|reason| Some(reason) == failure.reason)
            && self
                .provider_id
                .is_none_or(|provider| provider == failure.provider)
            && self
                .model
                .as_deref()
                .is_none_or(|model| model == failure.model)
            && self
                .purpose
                .is_none_or(|purpose| purpose == failure.purpose)
            && self
                .status
                .is_none_or(|status| Some(status) == failure.status)
            && self
                .contains
                .as_deref()
                .is_none_or(|words| failure.words.to_lowercase().contains(&words.to_lowercase()))
            && self.regex.as_deref().is_none_or(|pattern| {
                regex::RegexBuilder::new(pattern)
                    .case_insensitive(true)
                    .build()
                    .is_ok_and(|regex| regex.is_match(failure.words))
            })
    }
}

/// Progress is stored with the failed subject rather than reset on every turn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Cursor {
    /// The current step's index.
    pub step: u32,
    /// The attempts already made within that step.
    pub attempt: u32,
}

impl Cursor {
    /// Reserves the next permitted step and advances the durable cursor.
    pub fn next(&mut self, rule: &Rule) -> Option<(Step, u32)> {
        loop {
            let step = rule.steps.get(usize::try_from(self.step).ok()?)?.clone();
            let times = match &step {
                Step::Retry { times, .. } | Step::Continue { times } => *times,
                _ => 1,
            };
            if self.attempt >= times {
                self.step += 1;
                self.attempt = 0;
                continue;
            }
            self.attempt += 1;
            return Some((step, self.attempt));
        }
    }
}

/// The shipped rule values, editable through the ordinary inherited settings.
pub const DEFAULT_RULES: &str = r#"[
    {"id":"busy","when":{"kind":"provider_busy"},"steps":[{"type":"retry","delay_secs":60,"times":3,"backoff":2},{"type":"wait","until":"person"}]},
    {"id":"unreachable","when":{"kind":"provider_unreachable"},"steps":[{"type":"retry","delay_secs":60,"times":3,"backoff":2},{"type":"wait","until":"person"}]},
    {"id":"refused","when":{"kind":"model_refused"},"steps":[{"type":"continue","times":3},{"type":"wait","until":"person"}]},
    {"id":"limit","when":{"kind":"provider_exhausted"},"steps":[{"type":"wait","until":"reset"}]},
    {"id":"other","when":{},"steps":[{"type":"wait","until":"person"}]}
]"#;

/// Validates a rules value before it can become a scope's policy.
pub fn validate(value: &Json) -> Result<Vec<Rule>, String> {
    let rules: Vec<Rule> =
        serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
    let mut seen = std::collections::HashSet::new();
    for rule in &rules {
        if rule.id.trim().is_empty() || !seen.insert(&rule.id) || rule.steps.is_empty() {
            return Err("each rule needs a unique name and at least one step".to_owned());
        }
        if let Some(pattern) = &rule.when.regex {
            regex::RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .map_err(|error| error.to_string())?;
        }
        for step in &rule.steps {
            match step {
                Step::Retry { delay_secs, times, backoff } if *delay_secs < 0 || *times == 0 || *backoff == 0 => return Err("retry needs a nonnegative delay and positive times and backoff".to_owned()),
                Step::Continue { times: 0 } => return Err("continue needs positive times".to_owned()),
                Step::Failover { candidate } if candidate.model.trim().is_empty() || !candidate.params.is_object() || candidate.dimensions == Some(0) => return Err("a candidate needs a model, object parameters and positive dimensions when supplied".to_owned()),
                Step::Failover { candidate } => { requested_effort(&candidate.params).map_err(str::to_owned)?; }
                _ => {}
            }
        }
    }
    Ok(rules)
}

/// The one effort requested by neutral or documented protocol options, checked before current use.
pub fn requested_effort(params: &Json) -> Result<Option<&str>, &'static str> {
    let mut selected: Option<Option<&str>> = None;
    for path in [
        "/effort",
        "/reasoning/effort",
        "/reasoning_effort",
        "/output_config/effort",
    ] {
        let Some(value) = params.pointer(path) else {
            continue;
        };
        let value = if value.is_null() {
            None
        } else {
            Some(
                value
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .ok_or("effort must be text or null")?,
            )
        };
        if selected.is_some_and(|previous| previous != value) {
            return Err("conflicting effort parameters");
        }
        selected = Some(value);
    }
    Ok(selected.flatten())
}

/// Completion candidates may change generation options, never tools, history or retention controls.
pub fn completion_options(params: &Json) -> Result<(), &'static str> {
    let object = params
        .as_object()
        .ok_or("completion parameters must be an object")?;
    requested_effort(params)?;
    for (key, value) in object {
        match key.as_str() {
            "effort" | "reasoning_effort" => {}
            "reasoning" | "output_config" => {
                let nested = value
                    .as_object()
                    .ok_or("effort options must be an object")?;
                if nested.keys().any(|key| key != "effort") {
                    return Err(
                        "only a declared effort can change the completion's reasoning options",
                    );
                }
            }
            "temperature" | "top_p" if value.is_number() => {}
            _ => return Err("this completion option is not a supported model parameter"),
        }
    }
    Ok(())
}

/// A whole-second retry pause must never end before the vendor's absolute deadline.
#[must_use]
pub fn seconds_until(deadline: Time, now: Time) -> i64 {
    let millis = deadline
        .unix_millis()
        .saturating_sub(now.unix_millis())
        .max(0);
    (millis / 1_000).saturating_add(i64::from(millis % 1_000 != 0))
}

/// The pause for a retry, preferring the vendor's own time to the rule's growing fallback.
#[must_use]
pub fn retry_delay(step: &Step, attempt: u32, vendor_secs: Option<i64>) -> i64 {
    if let Some(secs) = vendor_secs.filter(|secs| *secs >= 0) {
        return secs;
    }
    match step {
        Step::Retry {
            delay_secs,
            backoff,
            ..
        } => {
            delay_secs.saturating_mul(i64::from(*backoff).saturating_pow(attempt.saturating_sub(1)))
        }
        _ => 0,
    }
}

/// Vectors may fail over only when their model identity and dimensions remain comparable.
#[must_use]
pub fn compatible_embedding(candidate: &Candidate, model: &str, dimensions: u32) -> bool {
    candidate.model == model && candidate.dimensions == Some(dimensions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_and_refused_get_three_attempts_but_structural_access_waits() {
        let rules = validate(&serde_json::from_str(DEFAULT_RULES).unwrap()).unwrap();
        for (kind, expected) in [
            ("provider_busy", "busy"),
            ("model_refused", "refused"),
            ("model_unavailable", "other"),
        ] {
            let rule = rules
                .iter()
                .find(|rule| {
                    rule.when.matches(&Match {
                        kind,
                        reason: None,
                        provider: Id::nil(),
                        model: "model",
                        purpose: Purpose::Agent,
                        status: None,
                        words: "",
                    })
                })
                .unwrap();
            assert_eq!(rule.id, expected);
            let mut cursor = Cursor::default();
            if expected == "other" {
                assert!(matches!(cursor.next(rule).unwrap().0, Step::Wait { .. }));
            } else {
                for attempt in 1..=3 {
                    assert_eq!(cursor.next(rule).unwrap().1, attempt);
                }
                assert!(matches!(cursor.next(rule).unwrap().0, Step::Wait { .. }));
            }
        }
    }

    #[test]
    fn vendor_delays_win_and_invalid_patterns_cannot_be_saved() {
        let step = Step::Retry {
            delay_secs: 60,
            times: 3,
            backoff: 2,
        };
        assert_eq!(retry_delay(&step, 3, None), 240);
        assert_eq!(retry_delay(&step, 3, Some(7)), 7);
        assert!(validate(&serde_json::json!([{"id":"new","when":{"regex":"["},"steps":[{"type":"wait","until":"person"}]}])).is_err());
    }
}

#[cfg(test)]
mod effort_options {
    use super::*;
    use serde_json::json;

    #[test]
    fn all_supported_effort_encodings_have_one_authorization_value() {
        for params in [
            json!({"effort":"high"}),
            json!({"reasoning":{"effort":"high"}}),
            json!({"reasoning_effort":"high"}),
            json!({"output_config":{"effort":"high"}}),
        ] {
            assert_eq!(requested_effort(&params), Ok(Some("high")));
        }
        assert_eq!(requested_effort(&json!({})), Ok(None));
        assert!(requested_effort(&json!({"effort":"low","reasoning":{"effort":"high"}})).is_err());
        assert!(requested_effort(&json!({"effort":false})).is_err());
    }
}

#[cfg(test)]
mod absolute_waits {
    use super::*;

    #[test]
    #[allow(clippy::expect_used, reason = "a test fails by panicking")]
    fn fractional_vendor_deadlines_round_up_and_past_deadlines_are_ready() {
        let now: Time = "2026-10-01T00:00:00.000Z"
            .parse()
            .expect("a fixed test timestamp");
        for (at, seconds) in [
            ("2026-10-01T00:00:00.001Z", 1),
            ("2026-10-01T00:00:01.000Z", 1),
            ("2026-10-01T00:00:01.001Z", 2),
            ("2026-09-30T23:59:59.999Z", 0),
        ] {
            assert_eq!(
                seconds_until(at.parse().expect("a fixed test timestamp"), now),
                seconds
            );
        }
    }
}

#[cfg(test)]
mod completion_options_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn completion_parameters_cannot_turn_a_summary_into_an_agent_or_change_retention() {
        for params in [
            json!({}),
            json!({"effort":"high","temperature":0.2}),
            json!({"reasoning":{"effort":"low"}}),
        ] {
            assert!(completion_options(&params).is_ok());
        }
        for params in [
            json!({"tools":[{"type":"web_search"}]}),
            json!({"store":true}),
            json!({"previous_response_id":"another-history"}),
            json!({"cyber_access_program":"unopened"}),
            json!({"reasoning":{"mode":"pro"}}),
        ] {
            assert!(completion_options(&params).is_err());
        }
    }
}

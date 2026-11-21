//! Captured summary observations are separate from current defaults and model admission.
use crate::usage::RequestTokens;
use crate::{
    common::Problem, forks::EstimateBasis, settings::SettingSource, summaries::SummaryProducerView,
    users::UserRef,
};
use orch_types::{Hash, Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Facts about the actual conformed output. A missing member was not captured, not measured zero.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SummaryResultFacts {
    /// Whitespace was normalized by the native field rule.
    pub flattened: Option<bool>,
    /// Only the first nonempty line of a multiline answer was kept.
    pub first_line_only: Option<bool>,
    /// Characters cut by the actual character allowance, after formatting.
    pub removed_characters: Option<u64>,
    /// Unknown tag names omitted from this answer.
    pub unknown_tags: Option<u64>,
    /// Already retained tag names omitted from this answer.
    pub duplicate_tags: Option<u64>,
    /// Known, nonduplicate tags beyond this answer's allowance.
    pub over_budget_tags: Option<u64>,
    /// The invalid handoff was rejected whole, without keeping raw text or a command prefix.
    pub rejected_whole: Option<bool>,
    /// Canonical measured request counts; unknown remains distinct from a real zero.
    pub captured_usage: Option<RequestTokens>,
}

/// A current observation for an explicit refresh, without reserving authority, credit or a model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SummaryRefreshProposal {
    /// Revision of the actual observed inputs and choice; not an admission token.
    pub revision: Hash,
    /// The proposed model, kept separate from the previous paragraph's producer.
    pub producer: SummaryProducerView,
    /// The actual manual requester whose usage applies, where still nameable.
    pub usage_owner: Option<UserRef>,
    /// Rendered prompt estimate; never substituted from a quote or current paragraph length.
    pub input_estimate_tokens: Option<u32>,
    /// Actual request output reservation, including reasoning room where applicable.
    pub output_allowance_tokens: Option<i64>,
    /// Whether this quantity is a measurement or an estimate.
    pub estimate_basis: EstimateBasis,
    /// Existing safe native reason when this proposal is unavailable.
    pub problem: Option<Problem>,
}

/// The actual material of the last turn selected for one helper request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalHelperCoverage {
    /// No last-turn material was present in the request.
    None,
    /// The selected last turn was included whole.
    Included,
    /// The beginning was left out to fit the actual allowance.
    StartOmitted,
    /// The turn exceeded the captured per-turn byte allowance.
    Oversized,
    /// The turn exceeded the chosen item allowance.
    ItemsExceeded,
}

/// One captured helper attempt/result; absence of a member stays unreported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GoalHelperReceipt {
    /// Whether a real helper answer was interpreted, separate from an admitted/pending attempt.
    pub checked: Option<bool>,
    /// Actual model call identity, not a later loop's attempt.
    pub attempt_id: Id,
    /// Safe model facts; provider configuration is independent.
    pub producer: Option<SummaryProducerView>,
    /// Actual admitted usage attribution, not inferred from the goal setter.
    pub usage_owner: Option<UserRef>,
    /// Actual admitted input allowance.
    pub input_allowance_tokens: Option<i64>,
    /// Actual admitted output allowance.
    pub output_allowance_tokens: Option<i64>,
    /// Captured coverage; absent for an older attempt without that observation.
    pub last_turn: Option<GoalHelperCoverage>,
    /// Actual canonical reported counts, retaining measured zero.
    pub usage: Option<RequestTokens>,
    /// When the admitted facts were captured.
    pub captured_at: Time,
}

/// The exact publication predicate that discarded automatic work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SummaryDiscardReason {
    /// The authority generation changed.
    AccessChanged,
    /// The selected continuation changed.
    HistoryChanged,
    /// The source revision changed.
    SourceChanged,
    /// The configured field producer changed.
    ProducerChanged,
    /// The subject was archived.
    Archived,
    /// A person's field ownership wins.
    ChangedByPerson,
}

/// What actually happened to one generated automatic field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SummaryAttemptOutcome {
    /// The result passed publication; an identical value need not be written again.
    Published,
    /// A current publication predicate discarded the old result.
    Discarded,
    /// Native conformance rejected the answer.
    Unusable,
    /// The producer failed to answer.
    Failed,
}

/// One field's retained native outcome, independent of its current value and ownership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SummaryAttemptFacts {
    /// Safe actual usage attribution, when captured and still nameable.
    pub usage_owner: Option<UserRef>,
    /// The actual publication/output outcome.
    pub outcome: SummaryAttemptOutcome,
    /// Present only where the deciding predicate distinguishes the cause.
    pub discard_reason: Option<SummaryDiscardReason>,
    /// Safe recorded producer, never filled from a later choice.
    pub producer: Option<SummaryProducerView>,
    /// Canonical actual reported counts; discarded work remains counted.
    pub captured_usage: Option<RequestTokens>,
    /// Actual transformations when captured.
    pub result_facts: Option<SummaryResultFacts>,
    /// Actual attempt completion.
    pub at: Time,
}

/// A native scheduling reason, never parsed from an English error sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AutomaticSkipReason {
    /// No producer was selected.
    NoProducer,
    /// The admitted usage cap had no room.
    CapNoRoom,
    /// Current source/model access was unavailable.
    NotAllowed,
    /// The provider could not be used.
    ProviderUnavailable,
    /// The call rules reserved no automatic continuation.
    CallRulePaused,
    /// The selected source was unavailable.
    SourceUnavailable,
    /// The producer does not write this field.
    Unsupported,
    /// The producer failed.
    Failed,
}

/// Current durable queue state; a due instant is eligibility, not a promised start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AutomaticScheduleState {
    /// Actual accepted work is queued or claimed.
    Queued,
    /// An actual failed attempt has a retained retry eligibility.
    Retrying,
    /// The last attempt was skipped/paused without current queued work.
    Skipped,
    /// No work is currently queued.
    Idle,
}

/// One field's actual durable timing and the policy captured when it was calculated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AutomaticFieldSchedule {
    /// Current native queue phase.
    pub state: AutomaticScheduleState,
    /// Structured deciding cause, or unreported for older/supplier-only observations.
    pub reason: Option<AutomaticSkipReason>,
    /// Recorded trigger due time.
    pub due_at: Option<Time>,
    /// Recorded interval/retry eligibility, independent of last success.
    pub next_at: Option<Time>,
    /// Actual last attempt.
    pub last_attempt_at: Option<Time>,
    /// This field's last actual successful write, not another field's or attempt's time.
    pub last_success_at: Option<Time>,
    /// Retry ceiling applied to these recorded timings.
    pub retry_ceiling: Option<u64>,
    /// Its captured origin, named through current permitted source references.
    pub source: Option<SettingSource>,
}

/// Facts of one automatic field; empty values remain unknown rather than successful or Off.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AutomaticFieldFacts {
    /// Actual last producer/publication disposition, where captured.
    pub last_attempt: Option<SummaryAttemptFacts>,
    /// This field's current durable work and captured timing policy.
    pub schedule: Option<AutomaticFieldSchedule>,
}

/// The two field owners stay independent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AutomaticFields {
    /// Title ownership and attempts stay independent of tags.
    pub title: AutomaticFieldFacts,
    /// Tag ownership and attempts stay independent of title.
    pub tags: AutomaticFieldFacts,
}

/// Inclusive native ordinals; adjacent ranges can be retained without enumerating every turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SummaryTurnRange {
    /// First native ordinal, inclusive.
    pub first: i64,
    /// Last native ordinal, inclusive.
    pub through: i64,
}

/// The actual bounded handoff input, separate from the full transcript and resource-summary items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HandoffInputCoverage {
    /// The per-turn read allowance applied while choosing this material.
    pub read_turn_bytes: u64,
    /// Actual whole-turn text selected for the prompt.
    pub included_turn_ranges: Vec<SummaryTurnRange>,
    /// Turns omitted without reading their bodies because their stored size exceeded that allowance.
    pub oversized_turn_ranges: Vec<SummaryTurnRange>,
    /// Earlier selected history remained outside this bounded material.
    pub omitted_prefix: bool,
    /// The source/material decision time, not the time a later viewer opens it.
    pub captured_at: Time,
}

/// Captured alongside the retained handoff/change outcome, without rejected raw model output.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HandoffResult {
    /// Actual model input coverage, independent of the complete transcript file.
    pub input_coverage: Option<HandoffInputCoverage>,
    /// Conformance and canonical reported usage of the real output.
    pub result_facts: Option<SummaryResultFacts>,
}

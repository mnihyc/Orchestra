//! The usage ledger read at three depths (one's own, a place's totals, a place's reports): figures, series over a span split by a dimension, and records, in tokens and credits; the rate card and the catalogue of estimates behind it, with which of them a figure came from; caps on tokens, on credits and on turns running at once; and the limits in a turn's way, with who may let it past each.
//!
//! Every use is a record in the ledger, and what these requests return is
//! a view of those records: [`UsageAnalytics`] adds them up, [`UsageEvent`]s
//! are the records themselves. What a caller is shown depends on the
//! [`UsageDepth`] their access gives them. Credits are an estimate of what
//! was used, in the unit the rates define; Orch does no billing. A
//! [`UsageCap`] bounds tokens, credits or turns running at once, and a
//! [`CapStatus`] says how much of it is used and whether it stands in the
//! way.

use crate::operations::{
    FacetSpec as Facet, OperationSpec as Op, TargetResolver as Target, VariantSpec as Variant,
};
use orch_access_contract::key_scope::KeyAction::{Manage, Read};
use orch_access_contract::key_scope::KeyIdentityKind as I;
use orch_access_contract::key_scope::KeyTargetKind::{Identity, Object};
use orch_access_contract::permissions::ObjectKind as K;

use orch_types::{Id, Revision, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::Role;
use crate::binding::ArrangementType;
use crate::common::Page;
use crate::directory::Group;
use crate::environment::EnvironmentRef;
use crate::nodes::NodeRef;
use crate::resources::{AgentRef, ProviderRef, Rate};
use crate::routes::Access::{Rule, Session, User};
use crate::threads::TokenUsage;
use crate::users::UserRef;

/// How many rows of each breakdown a usage report returns when it is not told.
pub const USAGE_BREAKDOWN_LIMIT_DEFAULT: u32 = 10;
/// The most rows of each breakdown a usage report returns.
pub const USAGE_BREAKDOWN_LIMIT_MAX: u32 = 100;
/// How many of the largest keys a series split by a dimension names; the rest are one key of
/// nothing.
pub const USAGE_SERIES_KEYS_MAX: u32 = 8;
/// A span of up to this many days is read by the hour when no bucket is asked for.
pub const USAGE_BUCKET_HOUR_UP_TO_DAYS: u32 = 2;
/// A span of up to this many days is read by the day when no bucket is asked for.
pub const USAGE_BUCKET_DAY_UP_TO_DAYS: u32 = 92;
/// A span of up to this many years is read by the week when no bucket is asked for; a longer one
/// by the month.
pub const USAGE_BUCKET_WEEK_UP_TO_YEARS: u32 = 2;
/// The longest span, in days, that a bucket by the hour is allowed for.
pub const USAGE_HOUR_BUCKET_MAX_DAYS: u32 = 31;
/// The longest span, in days, that a bucket by the day is allowed for, so that no answer holds
/// more than some 800 points.
pub const USAGE_DAY_BUCKET_MAX_DAYS: u32 = 800;
/// The most bytes of a rate catalogue that is given as a body.
pub const USAGE_CATALOGUE_MAX_BYTES: i64 = 2 * 1024 * 1024;
/// The fewest percent of a cap's amount at which a warning is given.
pub const CAP_WARN_PERCENT_MIN: u32 = 1;
/// The most percent of a cap's amount at which a warning is given.
pub const CAP_WARN_PERCENT_MAX: u32 = 99;
/// The most decimal places of an amount in credits.
pub const CAP_CREDIT_DECIMALS: u32 = 6;

/// Whether a cap's amount is shared or given to each person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapSplit {
    /// One amount for all of them together.
    Shared,
    /// That amount for each person.
    Each,
}

/// What a cap counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapMeter {
    /// Tokens, as whole numbers.
    Tokens,
    /// Credits, with at most six decimal places.
    Credits,
    /// Turns under way at the same time. It has an amount that is a whole number of turns and no
    /// period, warning, `at_limit` or `uncounted`.
    Running,
}

/// The period a cap counts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapPeriod {
    /// A day.
    Day,
    /// A week.
    Week,
    /// A month.
    Month,
    /// All the time, from `counted_from`.
    Total,
}

/// What happens to a turn under way when a cap is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapAtLimit {
    /// The turn is finished and no further one starts.
    FinishTurn,
    /// The turn is stopped.
    Stop,
}

/// What a cap does with use that cannot be counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapUncounted {
    /// Work goes on.
    Allow,
    /// Work is held back.
    Block,
}

/// Whom a cap covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CapSubject {
    /// Members whose domain is this org or an org inside it.
    Members { node: NodeRef },
    /// Everybody.
    Everyone,
    /// One person.
    User {
        /// The person.
        user: UserRef,
    },
    /// A group of the cap's tree of orgs, or, for a provider's cap, of the tree the provider's home
    /// is in.
    Group {
        /// The group.
        group: Group,
    },
    /// The people whose role at the thread is that one, and not those above it.
    Role {
        /// The role.
        role: Role,
        /// Where the role is held; inherited down from above as on permission pages.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<NodeRef>,
    },
}

/// A limit on use: of tokens, of credits, or of turns running at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageCap {
    /// The cap's identifier.
    pub id: Id,
    /// The place it is defined on.
    pub node: Option<NodeRef>,
    /// The provider it is defined on. Both `null` is the whole system.
    pub provider: Option<ProviderRef>,
    /// Whom it covers.
    pub subject: CapSubject,
    /// One amount for all of them together, or that amount per person.
    pub split: CapSplit,
    /// Model names, patterns with `*` for a family, and `@<group>` for the models that a provider's
    /// managers put in that group; empty for every model.
    pub models: Vec<String>,
    /// Service offering names, families and @groups; empty with models empty covers every offering.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
    /// Usage purposes, intersected with the offering selection; empty covers every purpose.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub purposes: Vec<String>,
    /// The service tiers it covers, by id (`fast`); empty for whatever tier a turn runs with, the
    /// ordinary one included.
    pub tiers: Vec<String>,
    /// The agents it covers, by harness kind; empty for every agent.
    pub harness_kinds: Vec<String>,
    /// The same agents, named; absent for every agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harnesses: Vec<crate::resources::HarnessKindName>,
    /// What it counts.
    pub meter: CapMeter,
    /// The period it counts in; `null` for `running`.
    pub period: Option<CapPeriod>,
    /// For the period `total`: a date; `null` for from the beginning.
    pub counted_from: Option<String>,
    /// A decimal string: whole tokens, credits, or a number of turns; `null` for no limit for
    /// these people.
    pub amount: Option<String>,
    /// What happens to a turn under way when it is reached.
    pub at_limit: CapAtLimit,
    /// The percent of the amount at which a warning is given, from [`CAP_WARN_PERCENT_MIN`] to
    /// [`CAP_WARN_PERCENT_MAX`]; `null` for none.
    pub warn_percent: Option<u32>,
    /// What it does with use that cannot be counted.
    pub uncounted: CapUncounted,
    /// A note of whoever set it.
    pub note: String,
    /// Whether it is in force.
    pub enabled: bool,
    /// The revision, which a change names as `expected_revision`.
    pub revision: Revision,
    /// Who made it.
    pub created_by: UserRef,
    /// When it was made.
    pub created_at: Time,
    /// When it was last changed.
    pub updated_at: Time,
    /// Whether the caller may change it.
    pub can_edit: bool,
}

/// Whom a cap that is being made covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CapSubjectInput {
    /// Members whose domain is this org or an org inside it.
    Members { id: Id },
    /// Everybody.
    Everyone,
    /// One person.
    User {
        /// The person.
        id: Id,
    },
    /// A group.
    Group {
        /// The group.
        id: Id,
    },
    /// A role.
    Role {
        /// The role.
        role: Role,
        /// The place whose role is named; legacy requests without it use the work's place.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<Id>,
    },
}

/// A cap that is being made. Its node, provider, subject, split, meter and period are fixed when it
/// is created; to change one, another cap is added and this one removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapInput {
    /// The node to define it on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<Id>,
    /// The provider to define it on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<Id>,
    /// Whom it covers.
    pub subject: CapSubjectInput,
    /// One amount for all of them together, or that amount per person.
    pub split: CapSplit,
    /// Model names, families and `@<group>`; empty for every model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    /// Service offering names, families and @groups.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub services: Option<Vec<String>>,
    /// Usage purpose patterns; empty covers every purpose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purposes: Option<Vec<String>>,
    /// Service tiers by id; empty for whatever tier, the ordinary one included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiers: Option<Vec<String>>,
    /// Harness kinds; empty for every agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_kinds: Option<Vec<String>>,
    /// What it counts.
    pub meter: CapMeter,
    /// The period; required unless `meter` is `running`, which has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<CapPeriod>,
    /// For the period `total`: a date; from the beginning when left out or `null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counted_from: Option<String>,
    /// A decimal string; `null` for no limit for these people, which is how one person or one role
    /// is exempted from an allowance that covers everyone.
    pub amount: Option<String>,
    /// What happens to a turn under way; `stop` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_limit: Option<CapAtLimit>,
    /// The percent at which a warning is given; none when left out or `null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warn_percent: Option<u32>,
    /// What it does with use that cannot be counted; `allow` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncounted: Option<CapUncounted>,
    /// A note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// How far a cap is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapState {
    /// There is room.
    Ok,
    /// The warning percent was passed.
    Warning,
    /// There is no room.
    Reached,
}

/// What a cap that counts credits leaves out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapLeftOut {
    /// For caps in credits: tokens of models without a rate.
    pub unrated_tokens: i64,
    /// Service requests whose measured quantity or entered quote could not be priced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unrated_services: Option<i64>,
    /// Covered threads in which nobody reports usage.
    pub unmetered_threads: i64,
}

/// The last time a turn was let past a cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapBypassLast {
    /// Who let it past.
    pub by: UserRef,
    /// When.
    pub at: Time,
}

/// How often turns were let past a cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapBypasses {
    /// The turns let past it in the current period; for `running`, in the last day.
    pub count: i64,
    /// The latest.
    pub last: Option<CapBypassLast>,
}

/// How much of a cap is used, for one person where it is counted per person.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CapStatus {
    /// Projection at this period's observed pace; none for total and concurrent-turn caps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forecast: Option<orch_provider_contract::forecast::UsageForecast>,
    /// The cap.
    pub cap: UsageCap,
    /// Whose share this is, for a cap counted per person.
    pub person: Option<UserRef>,
    /// What is counted in the current period, in the cap's unit; for `running`, the covered turns
    /// under way now.
    pub used: String,
    /// What covered turns that are still running reported so far; `0` for `running`.
    pub in_progress: String,
    /// For `running`: threads and held inputs that wait for room in this cap.
    pub waiting: i64,
    /// What the count leaves out.
    pub left_out: CapLeftOut,
    /// How far it is used.
    pub state: CapState,
    /// When the period ends; `null` for `total` and for `running`.
    pub period_ends_at: Option<Time>,
    /// For a cap counted per person: another cap applies to this person instead.
    pub replaced_by: Option<Id>,
    /// Whether the caller holds the bypass row where this cap is defined.
    pub can_bypass: bool,
    /// How often turns were let past it.
    pub bypasses: CapBypasses,
}

/// What a usage figure can be split by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageDimension {
    /// The person who used it.
    Person,
    /// The model, by its routing name.
    Model,
    /// Actual model or service offering, with its kind kept distinct.
    Offering,
    /// What the request was spent on.
    Purpose,
    /// The provider it ran on.
    Provider,
    /// The harness kind, such as `codex` or `claude-code`.
    Agent,
    /// The kind of place the tools ran in: a unit made from a template (`managed`) or a person's
    /// machine (`connected`).
    Environment,
    /// The reasoning effort.
    Effort,
    /// The service tier.
    Tier,
    /// The access program, where one was selected.
    Program,
    /// The orgs directly inside the one asked about.
    Org,
    /// The project a topic lies in directly, a project of an org or a folder: the nearest.
    Project,
    /// The topic.
    Topic,
    /// The thread.
    Thread,
}

/// The kind of place the tools ran in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageEnvironmentKind {
    /// A unit made from a template.
    Managed,
    /// A person's machine.
    Connected,
}

/// Actual service quantities and the evidence coverage behind their sum.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ServiceUnitTotals {
    /// Sum of the actually reported nonnegative quantities.
    pub quantity: i64,
    /// Requests with a reported quantity, including explicit zero.
    pub known_requests: i64,
    /// Requests whose quantity is missing or invalid; none is guessed from the result list.
    pub unknown_requests: i64,
}

/// Counted consumption, with distinct turns from every matching historical record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageFigures {
    /// Distinct turns in the matching historical records, including turns with no token report.
    /// Multiple model requests or meters of the same turn count once within each group.
    pub turns: i64,
    /// Separate actual service units, never converted to token counts or guessed from rows/results.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub service_units: std::collections::BTreeMap<String, ServiceUnitTotals>,
    /// Input with its cached part, output with its reasoning part, total.
    pub tokens: TokenUsage,
    /// A decimal: the sum over the use that had a rate. Use without one adds nothing here.
    pub credits: String,
    /// The part of `credits` counted at the shipped catalogue's rates: an estimate that nobody here
    /// set.
    pub estimated_credits: String,
    /// What that sum leaves out: tokens of models without a rate.
    pub unrated_tokens: i64,
    /// Service requests whose measured quantity or entered quote could not be priced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unrated_services: Option<i64>,
    /// Request performance, kept apart from records whose source cannot measure it.
    #[serde(default, skip_serializing_if = "UsagePerformance::is_empty")]
    pub performance: UsagePerformance,
}

/// The selected service's immutable unit quote, distinct from a vendor-reported invoice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ServiceUnitPrice {
    /// Exact decimal amount per measured unit.
    pub amount: String,
    /// The original quote unit, such as credits or a catalogue currency.
    pub currency: String,
    /// What the quoted amount buys; kept even if it does not match the measured unit.
    pub unit: String,
}

/// Actual service request facts carried by the existing request measurement envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ServiceUsage {
    /// The exact selected service offering; not a generation-model alias.
    pub offering: String,
    /// Declared charged unit, such as call, page or image.
    pub unit: String,
    /// Actual native quantity; missing is never a guessed successful call.
    pub quantity: UsageMeasurement<i64>,
    /// Original trusted offering quote, copied before the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<ServiceUnitPrice>,
    /// Applied credits per unit, after the existing catalogue scale where applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits_per_unit: Option<String>,
}

/// One measured member, with the same four states as provider facts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UsageMeasurement<T> {
    /// This source cannot measure the member.
    #[default]
    Unsupported,
    /// Supported, but not read yet (including a running request).
    Unread,
    /// Read, with no value (for example an error before any output token).
    Empty,
    /// Measured, including a real zero.
    Told {
        /// The measured value in the member's stated unit.
        value: T,
    },
}

impl<T> UsageMeasurement<T> {
    /// Whether this quantity has no declared support, so older wire payloads may omit it.
    #[must_use]
    pub const fn is_unsupported(&self) -> bool {
        matches!(self, Self::Unsupported)
    }

    /// The reported value, only where measured.
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Told { value } => Some(value),
            _ => None,
        }
    }
}

/// A charge the vendor actually reported, kept in its original unit for audit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportedCost {
    /// Exact nonnegative decimal text, without a floating-point conversion.
    pub amount: String,
    /// Original unit from its extension; unknown when that source did not declare one.
    pub unit: Option<String>,
}

/// Public native price terms selected on the actual request; a present null keeps its price unknown.
pub type NativePrices = std::collections::BTreeMap<
    String,
    Option<orch_provider_contract::capabilities::pricing::NativePrice>,
>;

/// Request times exclude tools and human waits; milliseconds are whole nonnegative numbers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RequestTiming {
    /// Refusal or vendor-specific reason, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_reason: Option<String>,
    /// The HTTP status actually received; absent for a transport failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_status: Option<i64>,
    /// One-based attempt within the model step, when observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<i64>,
    /// Upstream request identity, separate from an adapter's own request reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,

    /// Vendor-reported native searches, separate from client tool calls and tokens.
    pub web_searches: UsageMeasurement<i64>,
    /// Successful native page fetches; unknown, unsupported and zero remain distinct.
    #[serde(default, skip_serializing_if = "UsageMeasurement::is_unsupported")]
    pub web_fetches: UsageMeasurement<i64>,
    /// Other declaration-named actual usage quantities; no result/item count is inferred.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub declared_counters: std::collections::BTreeMap<String, UsageMeasurement<i64>>,
    /// Selected declaration-owned quotes from the final native request; null is explicitly unknown.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub native_prices: NativePrices,
    /// Actual service offering/unit facts; absent on ordinary model requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_usage: Option<ServiceUsage>,
    /// Actual vendor charge in its declared original unit; absence never implies zero or a credit conversion.
    #[serde(default, skip_serializing_if = "UsageMeasurement::is_unsupported")]
    pub reported_cost: UsageMeasurement<ReportedCost>,
    /// The actual upstream wire format, where this observation retained it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_protocol: Option<String>,
    /// The processing tier requested by this model client, where known.
    pub requested_service_tier: Option<String>,
    /// The tier or speed the upstream actually reported; absence never proves ordinary speed.
    pub reported_service_tier: UsageMeasurement<String>,
    /// Capacity is independent of inference speed; omitted by older/unaware sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_capacity_tier: Option<String>,
    /// The actual capacity class, never inferred from the requested automatic selection.
    #[serde(default, skip_serializing_if = "UsageMeasurement::is_unsupported")]
    pub reported_capacity_tier: UsageMeasurement<String>,
    /// One-hour cache writes inside the inclusive cache-write count; absent is not measured zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h_tokens: Option<UsageMeasurement<i64>>,
    /// Twelve-hour cache writes inside the inclusive count, independently measured from shorter writes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_12h_tokens: Option<UsageMeasurement<i64>>,
    /// A child model call made while a tool/sub-agent runs, excluded from the main-stream time sum.
    pub subagent: bool,
    /// Raw observed boundaries retained for turn ordering and diagnostics.
    pub asked: Option<Time>,
    /// First output event, where observed.
    pub first_output: Option<Time>,
    /// Last output boundary, where observed.
    pub end: Option<Time>,
    /// Counts the source reported; null members are not measured, including in a running request.
    pub reported_tokens: Option<RequestTokens>,
    /// Whether token-level output was streamed, where the source knows.
    pub output_streamed: Option<bool>,
    /// The vendor's own error words, where supplied.
    pub vendor_words: Option<String>,
    /// Streamed output tokens per second in thousandths, after the shared sample-eligibility rule.
    pub speed_milli: UsageMeasurement<i64>,
    /// Output tokens less unstreamed reasoning; unsupported when hidden reasoning cannot be separated.
    pub streamed_tokens: UsageMeasurement<i64>,
    /// Thinking was not streamed; its time is inside time to first output.
    pub reasoning_hidden: bool,
    /// Request sent to the first output token, reasoning and tool-call tokens included.
    pub first_token_ms: UsageMeasurement<i64>,
    /// First to last output token; never the time spent running tools.
    pub generation_ms: UsageMeasurement<i64>,
    /// Whole model request time, or the harness's API time for a turn fallback.
    pub request_ms: UsageMeasurement<i64>,
    /// `done`, or an existing turn error code; unread while running.
    pub outcome: UsageMeasurement<String>,
    /// Safe words supplied for the outcome, where present.
    pub error_message: Option<String>,
}

impl RequestTiming {
    /// No retained request facts or measured members. Tier-only observations remain
    /// serializable without claiming token/timing measurement capability.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether this source declares no measured members.
    pub fn is_unsupported(&self) -> bool {
        // Request-option facts cannot manufacture token/timing measurement capability.
        Self {
            error_reason: None,
            upstream_status: None,
            attempt: None,
            provider_request_id: None,
            web_searches: UsageMeasurement::Unsupported,
            web_fetches: UsageMeasurement::Unsupported,
            declared_counters: Default::default(),
            native_prices: Default::default(),
            service_usage: None,
            reported_cost: UsageMeasurement::Unsupported,
            wire_protocol: None,
            requested_service_tier: None,
            reported_service_tier: UsageMeasurement::Unsupported,
            requested_capacity_tier: None,
            reported_capacity_tier: UsageMeasurement::Unsupported,
            cache_write_1h_tokens: None,
            cache_write_12h_tokens: None,
            ..self.clone()
        } == Self::default()
    }
}

/// Counts a request source reports, with unknown kept apart from a measured zero.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RequestTokens {
    /// Input, including cached and cache-write parts.
    pub input_tokens: Option<i64>,
    /// Input read from the cache.
    pub cached_input_tokens: Option<i64>,
    /// Input written to the cache.
    pub cache_write_tokens: Option<i64>,
    /// Output, including reasoning.
    pub output_tokens: Option<i64>,
    /// Reasoning counted separately, where reported.
    pub reasoning_tokens: Option<i64>,
    /// Total, where reported.
    pub total_tokens: Option<i64>,
}

impl From<TokenUsage> for RequestTokens {
    fn from(tokens: TokenUsage) -> Self {
        Self {
            input_tokens: Some(tokens.input_tokens),
            cached_input_tokens: Some(tokens.cached_input_tokens),
            cache_write_tokens: Some(tokens.cache_write_tokens),
            output_tokens: Some(tokens.output_tokens),
            reasoning_tokens: Some(tokens.reasoning_tokens),
            total_tokens: Some(tokens.total_tokens),
        }
    }
}

impl RequestTokens {
    /// No token count was reported; an explicit zero remains a known count.
    pub fn is_empty(&self) -> bool {
        [
            self.input_tokens,
            self.cached_input_tokens,
            self.cache_write_tokens,
            self.output_tokens,
            self.reasoning_tokens,
            self.total_tokens,
        ]
        .iter()
        .all(Option::is_none)
    }

    /// Known counts for the existing numeric ledger; the original nullable report is kept beside it.
    pub fn usage(&self) -> TokenUsage {
        let input = self.input_tokens.unwrap_or_default().max(0);
        let output = self.output_tokens.unwrap_or_default().max(0);
        TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cached_input_tokens: self.cached_input_tokens.unwrap_or_default().clamp(0, input),
            cache_write_tokens: self.cache_write_tokens.unwrap_or_default().clamp(0, input),
            reasoning_tokens: self.reasoning_tokens.unwrap_or_default().clamp(0, output),
            total_tokens: self
                .total_tokens
                .unwrap_or(input.saturating_add(output))
                .max(0),
        }
    }
}

/// Computation-only input; wire reports are owned by the adapter contract.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestSample {
    /// The request belongs to a tool/sub-agent rather than the turn's main stream.
    pub subagent: bool,
    /// The output count, where known.
    pub output_tokens: Option<i64>,
    /// Token-level output was observed, not a buffered complete message.
    pub output_streamed: Option<bool>,
    /// Original nullable counts, retained for API readers.
    pub reported_tokens: Option<RequestTokens>,
    /// The vendor's own words, where supplied.
    pub vendor_words: Option<String>,
    /// Input or a tool result given back, observed by the adapter; null when unavailable.
    pub asked: Option<Time>,
    /// First output event (Claude message_start, Codex first delta); null when unavailable or pending.
    pub first_output: Option<Time>,
    /// Last output boundary (Claude message_stop, Codex last output before a tool/turn end).
    pub end: Option<Time>,
    /// Whether reasoning was not streamed.
    pub reasoning_hidden: bool,
    /// The reasoning count, where the harness reports it separately; null does not mean zero.
    pub reasoning_tokens: Option<i64>,
    /// Unread while running, then done or an existing turn error code.
    pub outcome: UsageMeasurement<String>,
    /// Safe words accompanying an error.
    pub error_message: Option<String>,
}

/// Performance figures derived from mergeable request histograms.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct UsagePerformance {
    /// Completed requests with hidden reasoning, including those without an honest speed.
    pub hidden_requests: i64,
    /// Requests whose known stream was too short or too small for speed.
    pub short_requests: i64,
    /// Completed requests with a known outcome; turn fallbacks where requests cannot be seen.
    pub requests: i64,
    /// Provider-side failed requests by stable code; harness, environment and cancellation outcomes stay in records.
    pub errors: std::collections::BTreeMap<String, i64>,
    /// Median output speed, in thousandths of a token per second; null means not measured.
    pub speed_milli: Option<i64>,
    /// 90th percentile output speed in thousandths of a token per second.
    pub speed_p90_milli: Option<i64>,
    /// Median time to first output token, in milliseconds.
    pub first_token_ms: Option<i64>,
    /// 90th percentile time to first output token, in milliseconds.
    pub first_token_p90_ms: Option<i64>,
    /// Requests contributing a speed measurement.
    pub speed_samples: i64,
    /// Requests contributing a first-token measurement.
    pub first_token_samples: i64,
}

impl UsagePerformance {
    /// Whether there were no measured outcomes or times.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
    /// Provider failures only, including when reading older stored maps with other outcome codes.
    pub fn error_count(&self) -> i64 {
        self.errors
            .iter()
            .filter(|(code, _)| provider_error(code))
            .map(|(_, n)| *n)
            .sum()
    }
}

/// Provider-side outcomes used by Performance's error rate and provider health.
pub const PROVIDER_ERROR_CODES: [&str; 9] = [
    "provider_exhausted",
    "provider_unauthorized",
    "provider_busy",
    "rate_limited",
    "overloaded",
    "stream_interrupted",
    "provider_unreachable",
    "model_unavailable",
    "model_refused",
];

/// A harness/session/tool failure does not blame the model provider.
pub fn provider_error(code: &str) -> bool {
    PROVIDER_ERROR_CODES.contains(&code)
}

/// What the usage chart measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageMeasure {
    /// Turns.
    Turns,
    /// Completed requests with a known outcome.
    Requests,
    /// Total tokens.
    Tokens,
    /// Credits.
    Credits,
    /// Median output tokens per second.
    Speed,
    /// Median first-token milliseconds.
    FirstToken,
    /// Failed requests divided by measured requests.
    Errors,
}

/// The one outcome-filter vocabulary for figures, records and the live request log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageOutcomeFilter {
    /// Requests with no terminal report yet.
    Running,
    /// Failed outcomes, excluding completion and cancellation.
    Errors,
    /// No measured outcome (`outcome IS NULL`).
    NotMeasured,
    /// Completed normally.
    Done,
    /// Provider allowance exhausted.
    ProviderExhausted,
    /// Provider refused the credential.
    ProviderUnauthorized,
    /// Provider rate-limited or overloaded.
    ProviderBusy,
    /// The provider rate limit was reached.
    RateLimited,
    /// The provider has no serving capacity.
    Overloaded,
    /// A response failed after output began.
    StreamInterrupted,
    /// Provider could not be reached.
    ProviderUnreachable,
    /// Model not available.
    ModelUnavailable,
    /// The vendor filter refused the request.
    ModelRefused,
    /// Context exceeded the model's window.
    ContextOverflow,
    /// Tool environment could not be reached.
    EnvironmentUnreachable,
    /// Harness failed.
    HarnessError,
    /// Session was lost.
    SessionLost,
    /// A person cancelled the request.
    Forced,
    /// Harness startup timed out.
    StartTimeout,
    /// Orch usage cap reached.
    CapReached,
}

impl UsageOutcomeFilter {
    /// The completed code, excluding the three compound/missing/running selectors.
    #[must_use]
    pub const fn code(self) -> Option<&'static str> {
        Some(match self {
            Self::Running | Self::Errors | Self::NotMeasured => return None,
            Self::Done => "done",
            Self::ProviderExhausted => "provider_exhausted",
            Self::ProviderUnauthorized => "provider_unauthorized",
            Self::ProviderBusy => "provider_busy",
            Self::RateLimited => "rate_limited",
            Self::Overloaded => "overloaded",
            Self::StreamInterrupted => "stream_interrupted",
            Self::ProviderUnreachable => "provider_unreachable",
            Self::ModelUnavailable => "model_unavailable",
            Self::ModelRefused => "model_refused",
            Self::ContextOverflow => "context_overflow",
            Self::EnvironmentUnreachable => "environment_unreachable",
            Self::HarnessError => "harness_error",
            Self::SessionLost => "session_lost",
            Self::Forced => "forced",
            Self::StartTimeout => "start_timeout",
            Self::CapReached => "cap_reached",
        })
    }
}

/// One model, tier, effort and program combination observed in this scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageComparison {
    /// Routing model name.
    pub model: String,
    /// Service tier, or the ordinary tier when `tier_unknown` is false.
    pub tier: Option<String>,
    /// A reporting-aware reply omitted its actual tier after an explicit nonordinary request.
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub tier_unknown: bool,
    /// Reasoning effort, where selected.
    pub effort: Option<String>,
    /// Access program, where selected.
    pub program: Option<String>,
    /// Its performance over the span.
    pub performance: UsagePerformance,
}

/// An agent, by what the figures name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageKeyAgent {
    /// The harness kind.
    pub kind: String,
    /// The profile's name.
    pub name: String,
}

/// Actual offering identity; a service is never disguised as a generation model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageOffering {
    /// Open offering kind (generation model or service).
    pub kind: String,
    /// Native routing/offering name, as recorded.
    pub name: String,
}

/// What a figure of a breakdown or of a series is about: the member of the dimension, and nothing
/// else. A place the caller may not see is a redacted reference.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageKey {
    /// Actual model/service identity when splitting by offering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offering: Option<UsageOffering>,
    /// Actual purpose when splitting by purpose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// The access program; null means none selected.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<String>")]
    pub program: Option<Option<String>>,
    /// The person; `null` for use attributed to nobody.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<UserRef>")]
    pub person: Option<Option<UserRef>>,
    /// The model, by its routing name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// What to show for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_display: Option<String>,
    /// The provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderRef>,
    /// The agent.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<UsageKeyAgent>")]
    pub agent: Option<Option<UsageKeyAgent>>,
    /// The kind of environment.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<UsageEnvironmentKind>")]
    pub environment: Option<Option<UsageEnvironmentKind>>,
    /// The reasoning effort.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<String>")]
    pub effort: Option<Option<String>>,
    /// The service tier.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<String>")]
    pub tier: Option<Option<String>>,
    /// Distinguishes an unreported actual tier from the ordinary null tier.
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub tier_unknown: bool,
    /// The org; `null` for the projects that lie directly in the org asked about.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<NodeRef>")]
    pub org: Option<Option<NodeRef>>,
    /// The project the topic lies in directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<NodeRef>,
    /// The topic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<NodeRef>,
    /// The thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<NodeRef>,
}

/// How a span is cut into points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageBucket {
    /// By the hour.
    Hour,
    /// By the day, from UTC midnight.
    Day,
    /// By the week, from Monday.
    Week,
    /// By the month.
    Month,
}

/// What a caller was shown of usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageDepth {
    /// The caller's own use there.
    Own,
    /// The figures of the place: in total, over time, and by person, model, provider, agent, kind of
    /// environment, and the places inside it that the caller may see. Not by thread, and no record
    /// but the caller's own.
    Summary,
    /// Everybody's use there, by every dimension, and every record.
    Reports,
}

/// One key of a point of a series, with its figures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageSplit {
    /// What the figures are about.
    pub key: UsageKey,
    /// Its figures.
    pub figures: UsageFigures,
}

/// One point of a series.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageSeriesPoint {
    /// The start of the bucket, UTC.
    pub at: Time,
    /// Its figures.
    pub figures: UsageFigures,
    /// Its figures split by `series_by`: its [`USAGE_SERIES_KEYS_MAX`] largest keys over the span and
    /// the rest as one key of nothing; empty without it.
    pub by: Vec<UsageSplit>,
}

/// One row of a breakdown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageRow {
    /// What the figures are about.
    pub key: UsageKey,
    /// Its figures.
    pub figures: UsageFigures,
    /// Its share of `totals.credits`, or of its tokens where nothing had a rate.
    pub share: f64,
}

/// The totals split by one dimension, the largest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageBreakdown {
    /// The dimension.
    pub by: UsageDimension,
    /// The rows, at most `limit`.
    pub rows: Vec<UsageRow>,
    /// How many keys there are beyond `limit`.
    pub more: i64,
}

/// Usage over a span: totals, a series and breakdowns. A node-scoped answer covers the node's
/// subtree; use in a place below that the caller may not see is counted in the totals and in each
/// person's figures, and its key is a redacted reference. A provider-scoped answer covers every
/// thread that ran on it, of whatever org.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageAnalytics {
    /// All figures for the equally long preceding span, returned with the current report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub figures_before: Option<UsageFigures>,
    /// Current provider degradation, only when requested for a provider scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degradation: Option<UsageDegradation>,
    /// Performance over the equally long span immediately before this one.
    #[serde(default, skip_serializing_if = "UsagePerformance::is_empty")]
    pub performance_before: UsagePerformance,
    /// Model combinations observed here, with mergeable-histogram percentiles.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comparison: Vec<UsageComparison>,
    /// What was asked about: `me`, `system`, `node:` and an identifier, or `provider:` and an identifier.
    pub scope: String,
    /// The first day, UTC.
    pub from: String,
    /// The last day, UTC.
    pub to: String,
    /// As asked; unless given: by the hour for a span of up to two days, by the day up to 92, by the
    /// week up to two years, by the month beyond.
    pub bucket: UsageBucket,
    /// What the caller was shown.
    pub depth: UsageDepth,
    /// The sums over the whole span.
    pub totals: UsageFigures,
    /// The points, one for each bucket.
    pub series: Vec<UsageSeriesPoint>,
    /// The totals by dimension.
    pub breakdowns: Vec<UsageBreakdown>,
    /// For a place: the caller's own part of `totals`.
    pub me: Option<UsageFigures>,
}

/// A provider's recent request health compared with its own seven-day figures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageDegradation {
    /// Start of the recent fifteen-minute window.
    pub since: Time,
    /// Recent completed requests.
    pub recent: UsagePerformance,
    /// This provider's seven-day baseline.
    pub baseline: UsagePerformance,
    /// At least twenty percent errors over at least ten requests.
    pub errors: bool,
    /// Recent first-output median exceeds three times its baseline, with ten measured samples in each.
    pub first_output: bool,
}

/// Which program wrote a ledger row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageOrigin {
    /// The agent's harness reported it.
    Harness,
    /// The forwarder of the provider's runtime counted it.
    Forwarder,
    /// Orch itself used a model.
    Orch,
}

/// What a ledger row was spent on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsagePurpose {
    /// A turn of an agent.
    Turn,
    /// The summary of a topic.
    Summary,
    /// Vectors of an org's text, or of a query, for search by meaning.
    Embeddings,
    /// An agent permission review, accounted apart from its generation.
    Review,
    /// A native or selected service web search.
    WebSearch,
    /// A native or selected service page fetch.
    PageFetch,
    /// Image generation by the selected offering.
    ImageGeneration,
    /// A question explicitly issued by a person.
    Ask,
}

/// What a ledger row counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageEventScope {
    /// A whole turn.
    Turn,
    /// One request to a model.
    Request,
}

/// Where the rate of a ledger row came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RateSource {
    /// The provider's own rate for the model.
    Provider,
    /// The rate card of the system or of an org.
    RateCard,
    /// The shipped catalogue's estimate by model family.
    Catalogue,
    /// A vendor-reported total, converted only in the matching catalogue currency.
    Reported,
}

/// One row of the ledger: a use, with its thread and turn, what it ran on, the rate it was counted
/// at and its credits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageEvent {
    /// The thread owner captured with the record, distinct from its input author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_owner: Option<UserRef>,
    /// Native tool items attributed to this request; absent without a proven association.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<i64>,
    /// Sum of associated tool durations, absent while any duration is unreported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_ms: Option<i64>,
    /// The observed request times and outcome; unsupported is distinct from zero.
    #[serde(default, skip_serializing_if = "RequestTiming::is_empty")]
    pub timing: RequestTiming,
    /// The access program selected for this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_program: Option<String>,
    /// Whether the model request is still running.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub running: bool,
    /// The row's identifier.
    pub id: Id,
    /// When it happened.
    pub at: Time,
    /// Who reported it.
    pub origin: UsageOrigin,
    /// What it was spent on.
    pub purpose: UsagePurpose,
    /// What it counts.
    pub scope: UsageEventScope,
    /// Whether it is counted in the figures.
    pub counted: bool,
    /// The project the topic lies in directly, which the row names.
    pub project: NodeRef,
    /// The topic.
    pub topic: NodeRef,
    /// The thread; `null` for a topic's summary.
    pub thread: Option<NodeRef>,
    /// The turn, where the timeline opens; `null` when its thread is unreadable or for a topic's summary.
    pub turn_id: Option<Id>,
    /// The turn's number in its readable thread; `null` where no readable turn remains.
    pub turn_ordinal: Option<i64>,
    /// Who used it.
    pub user: Option<UserRef>,
    /// The provider it ran on.
    pub provider: ProviderRef,
    /// The agent profile.
    pub agent: Option<AgentRef>,
    /// The harness kind.
    pub harness_kind: Option<String>,
    /// The same harness, named.
    pub harness: Option<crate::resources::HarnessKindName>,
    /// The environment.
    pub environment: Option<EnvironmentRef>,
    /// The kind of environment.
    pub environment_kind: Option<UsageEnvironmentKind>,
    /// The arrangement type.
    pub arrangement: Option<ArrangementType>,
    /// The routing name the use ran on, as recorded.
    pub model: String,
    /// Its display name on that provider now.
    pub model_display: String,
    /// The reasoning effort.
    pub effort: Option<String>,
    /// The service tier.
    pub tier: Option<String>,
    /// A reporting-aware reply omitted its actual tier after an explicit nonordinary request.
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub tier_unknown: bool,
    /// The tokens.
    pub tokens: TokenUsage,
    /// The retained source reference: the adapter's id when known, otherwise the upstream reference.
    pub request_ref: Option<String>,
    /// How large the prompt was, where known.
    pub context_tokens: Option<i64>,
    /// The rate it was counted at, as when the row was written.
    pub rate: Option<Rate>,
    /// Where that rate came from; `catalogue` is an estimate by model family.
    pub rate_source: Option<RateSource>,
    /// Its credits; `null` when there was no rate, which a client shows as "-" and never as 0.
    pub credits: Option<String>,
}

/// What one model usable at a node would be counted at now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RateTableModel {
    /// The provider.
    pub provider: ProviderRef,
    /// The routing name.
    pub model_name: String,
    /// The name to show.
    pub display_name: String,
    /// Its rate; `null` when it has none.
    pub rate: Option<Rate>,
    /// Where that rate comes from.
    pub source: Option<RateSource>,
    /// The pattern of the card or the catalogue that matched.
    #[serde(rename = "match")]
    pub pattern: Option<String>,
    /// What was used of it without a rate, in the last 30 days.
    pub unrated_tokens: i64,
}

/// How the shipped catalogue counts at a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RateTableCatalogue {
    /// Whether use without another rate is estimated from it.
    pub on: bool,
    /// What one unit of it is worth in the node's credits, a decimal.
    pub scale: String,
    /// The date of its figures.
    pub as_of: String,
}

/// What each model usable at a node is counted at, where that rate comes from, and which have
/// none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RateTable {
    /// The models.
    pub models: Vec<RateTableModel>,
    /// The catalogue, as resolved at the node.
    pub catalogue: RateTableCatalogue,
}

/// Where a catalogue in force came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CatalogueOrigin {
    /// It came with a release.
    Shipped,
    /// An admin gave a file.
    Uploaded,
    /// It was fetched from the address in `usage.catalogue_url`.
    Fetched,
}

/// What models of a family cost, by the pattern of their names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RateFamily {
    /// The model's name: lower case, without what precedes its last `/`; `*` stands for any run of
    /// characters.
    #[serde(rename = "match")]
    pub pattern: String,
    /// Who sells it.
    pub vendor: String,
    /// The page its figures were read from.
    pub source: String,
    /// The day they were read.
    pub as_of: String,
    /// What a reader should know of this entry, in a sentence: a promotion that ends, a price by the
    /// hour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// What it costs, in the catalogue's unit.
    pub rate: Rate,
}

/// The shipped estimates of what models cost, as loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RateCatalogue {
    /// The format of the file it was read from.
    pub format: u32,
    /// When its figures were last read from their sources.
    pub as_of: String,
    /// What its numbers are in: US dollars at the vendors' list prices.
    pub unit: String,
    /// How it came to be in force.
    pub origin: CatalogueOrigin,
    /// Where it was fetched from, for `fetched`.
    pub source: Option<String>,
    /// When it was loaded.
    pub loaded_at: Time,
    /// Who loaded it; `null` when a start or a schedule did.
    pub loaded_by: Option<UserRef>,
    /// The date of the one the running release came with.
    pub shipped_as_of: String,
    /// The families.
    pub families: Vec<RateFamily>,
}

/// Every cap that covers work at a node, those on running turns among them. Caps counted per person
/// show the caller's share.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Limits {
    /// The caps.
    pub caps: Vec<CapStatus>,
}

/// Which order the records of a ledger are listed in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageEventOrder {
    /// The oldest first.
    Asc,
    /// The newest first.
    #[default]
    Desc,
}

/// A lightweight provider health read; it returns no chart, comparison or previous-span figures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageHealthGetQuery {
    /// The provider whose current health is read, requiring provider.view.
    pub provider_id: Id,
}

/// The usage to add up. `scope` is `me`, `node:` and an identifier, or `provider:` and an
/// identifier; a dimension the caller's depth does not give is refused in `series_by`, in
/// `breakdowns` and in the filters. Answered as `text/csv` instead of JSON when `Accept` asks for
/// it, one line for each point of the series.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UsageGetQuery {
    /// Includes recent degradation for a provider, independently of the selected historical span.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<bool>,
    /// Only this turn; requires the same depth as a thread breakdown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<Id>,
    /// Only work inside this org or department.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
    /// The chart's measure; the answer retains all measures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measure: Option<UsageMeasure>,
    /// Only this effort, tier, program or outcome; `none` selects no value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Only this service tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// Only reporting-aware requests whose actual tier was not reported; false selects known tiers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_unknown: Option<bool>,
    /// Only this access program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    /// Only this outcome code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<UsageOutcomeFilter>,
    /// What to add up: `me`, `node:<id>`, `provider:<id>`, or `system` under current system administration.
    pub scope: String,
    /// The first day, UTC, as `YYYY-MM-DD`.
    pub from: String,
    /// The last day, UTC, as `YYYY-MM-DD`.
    pub to: String,
    /// How to cut the span; chosen by its length when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket: Option<UsageBucket>,
    /// A dimension to split the series by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_by: Option<UsageDimension>,
    /// A comma list of dimensions to break the totals down by; by default `project`, `model`,
    /// `provider`, `agent` and `environment` for `me`, `person`, `model`, `provider`, `agent` and
    /// `environment` for a node, and `person`, `model`, `org` and `environment` for a provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakdowns: Option<String>,
    /// Rows of each breakdown: [`USAGE_BREAKDOWN_LIMIT_DEFAULT`] unless given, at most
    /// [`USAGE_BREAKDOWN_LIMIT_MAX`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Only the use of this person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<Id>,
    /// Only this model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Only this service offering, by its actual name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// Only this actual usage purpose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// Only what ran on this provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<Id>,
    /// Only this harness kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_kind: Option<String>,
    /// Only this kind of environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_kind: Option<UsageEnvironmentKind>,
    /// Only what was done in the topics that lie in this project directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<Id>,
    /// Only this topic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<Id>,
    /// Only this thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Id>,
}

/// The records behind a figure, with the same scope and filters. Answered as `text/csv` instead of
/// JSON when `Accept` asks for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UsageEventListQuery {
    /// Only this turn; requires the same depth as a thread breakdown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<Id>,
    /// Only work inside this org or department.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
    /// Shows canonical model requests instead of the accounting trace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests_only: Option<bool>,
    /// Only this effort; `none` selects no effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Only this service tier; `none` selects the ordinary tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// Only reporting-aware requests whose actual tier was not reported; false selects known tiers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_unknown: Option<bool>,
    /// Only this access program; `none` selects no program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    /// Only this outcome code, or `running`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<UsageOutcomeFilter>,
    /// Includes the requests still running; completed rows remain cursor-paged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_running: Option<bool>,
    /// What to list.
    pub scope: String,
    /// The first day, UTC, as `YYYY-MM-DD`.
    pub from: String,
    /// The last day, UTC, as `YYYY-MM-DD`.
    pub to: String,
    /// Only the use of this person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<Id>,
    /// Only this model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Only this service offering, by its actual name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// Only this actual usage purpose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// Only what ran on this provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<Id>,
    /// Only this harness kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_kind: Option<String>,
    /// Only this kind of environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_kind: Option<UsageEnvironmentKind>,
    /// Only what was done in the topics that lie in this project directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<Id>,
    /// Only this topic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<Id>,
    /// Only this thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Id>,
    /// Only the rows that are counted, or only those that are not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counted: Option<bool>,
    /// The order; `desc` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<UsageEventOrder>,
    /// How many entries, at most [`crate::common::MAX_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// The rates at one node, or the system's own rate-card configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UsageRateTableGetQuery {
    /// The node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<Id>,
    /// With no node, the system form; only a browser-session system administrator may read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<bool>,
}

/// The catalogue, or the entry that matches a model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UsageCatalogueGetQuery {
    /// A model's name: with it, the family that would match it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
}

/// A catalogue given as a file. It is checked whole before it replaces the one in force: its
/// `format`, every pattern and every rate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RateCatalogueFile {
    /// The format, which the checks are made by.
    pub format: u32,
    /// When its figures were last read from their sources.
    pub as_of: String,
    /// What its numbers are in.
    pub unit: String,
    /// The families.
    pub families: Vec<RateFamily>,
}

/// Has the system take the catalogue from `usage.catalogue_url`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogueFetch {
    /// Always `true`.
    pub fetch: bool,
}

/// Puts back the catalogue that the running release came with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogueShipped {
    /// Always `true`.
    pub shipped: bool,
}

/// What replaces the catalogue in force: a file of at most [`USAGE_CATALOGUE_MAX_BYTES`], a request
/// to fetch it, or a request to put back the shipped one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum AdminUsageCatalogueSetBody {
    /// Take it from the address in `usage.catalogue_url`.
    Fetch(CatalogueFetch),
    /// Put back the one the running release came with.
    Shipped(CatalogueShipped),
    /// The catalogue itself.
    File(RateCatalogueFile),
}

/// Whose limits are wanted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UsageLimitsGetQuery {
    /// The node.
    pub node_id: Id,
    /// Somebody else's share of the caps counted per person; needs `usage.view`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<Id>,
}

/// Which caps are wanted: exactly one of `node_id`, `provider_id` and `system=true`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UsageCapListQuery {
    /// The caps defined on this node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<Id>,
    /// The caps defined on this provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<Id>,
    /// The caps of the system.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<bool>,
    /// With `node_id`: also the caps defined elsewhere that cover work there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub covering: Option<bool>,
}

/// One that waits for room in a cap on running turns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageCapWaiting {
    /// The thread; `null` for one the caller may not view.
    pub thread: Option<NodeRef>,
    /// Whose input it is.
    pub author: Option<UserRef>,
    /// Since when it waits.
    pub since: Time,
    /// Whether it is the caller's own.
    pub mine: bool,
}

/// What waits for room in a cap on running turns, in the order it will run: blocked threads, and for
/// a cap counted per person the inputs held for their author. Somebody the cap only covers sees
/// their own rows named and the others as places in the queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageCapWaitingListReply {
    /// Those that wait.
    pub items: Vec<UsageCapWaiting>,
}

/// Changes a cap. A cap's node, provider, subject, split, meter and period are fixed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UsageCapUpdateBody {
    /// New model names, families and `@<group>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    /// Service offering names, families and @groups.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub services: Option<Vec<String>>,
    /// Usage purpose patterns; empty covers every purpose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purposes: Option<Vec<String>>,
    /// New service tiers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiers: Option<Vec<String>>,
    /// New harness kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_kinds: Option<Vec<String>>,
    /// A new date to count from; `null` for from the beginning.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<String>")]
    pub counted_from: Option<Option<String>>,
    /// A new amount; `null` for no limit for these people.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<String>")]
    pub amount: Option<Option<String>>,
    /// What happens to a turn under way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_limit: Option<CapAtLimit>,
    /// A new warning percent; `null` for none.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<u32>")]
    pub warn_percent: Option<Option<u32>>,
    /// What it does with use that cannot be counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncounted: Option<CapUncounted>,
    /// A new note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Whether it is in force.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// The revision the caller saw.
    pub expected_revision: Revision,
}

/// A completed cap mutation without stored cap content or subject metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageCapReceipt {
    /// This acknowledgement intentionally carries no full projection.
    pub projection: crate::common::ReceiptProjection,
    /// The cap actually created or changed.
    pub id: Id,
    /// Its accepted revision.
    pub revision: Revision,
}
crate::common::full_or_receipt! {
    /// Native cap content or the same completed mutation's acknowledgement.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
    pub enum UsageCapWriteResponse {
        /// Unchanged native full cap, admitted by this operation's Read.
        Full(UsageCap),
        /// The accepted effect without stored cap metadata.
        Receipt(UsageCapReceipt),
    }
}

const CAP_SELECTED: Facet = Facet::required(
    Read,
    &[
        Object(K::Org),
        Object(K::Project),
        Object(K::Topic),
        Object(K::Thread),
        Object(K::Provider),
        Identity(I::System),
    ],
    Target::ResolvedCollection("caps"),
);
const CAP_GROUPS: Facet = Facet::required(
    Read,
    &[Identity(I::Group)],
    Target::ResolvedCollection("cap_groups"),
);
const USAGE_NAMES: Facet = Facet::required(
    Read,
    &[
        Object(K::Org),
        Object(K::Project),
        Object(K::Topic),
        Object(K::Thread),
    ],
    Target::ResolvedCollection("usage_names"),
);

crate::endpoints! {
    /// Current provider health, without chart, comparison or previous-span reads.
    UsageHealthGet: GET "/usage/health", Rule("provider.view for provider_id"), query UsageHealthGetQuery, none, json UsageDegradation, 200,
        operation Op::single(Read, &[Object(K::Provider)], Target::Resolved("provider"));
    /// Adds up usage over a span: totals, a series and breakdowns, as deep as the caller's access for the scope allows.
    UsageGet: GET "/usage", Rule("by scope: me needs sign-in; node:<id> needs usage.view, usage.summary or node.view there; provider:<id> needs provider.view; system needs current system administration"), query UsageGetQuery, none, json UsageAnalytics, 200,
        operation Op::single(
            Read,
            &[
                Object(K::Org),
                Object(K::Project),
                Object(K::Topic),
                Object(K::Thread),
                Object(K::Provider),
                Identity(I::Account),
                Identity(I::System),
            ],
            Target::Resolved("scope"),
        );
    /// Lists the ledger rows behind a figure, each with its thread and turn, what it ran on, the rate it was counted at and its credits.
    UsageEventList: GET "/usage/events", Rule("by scope, as for the figures"), query UsageEventListQuery, none, json Page<UsageEvent>, 200,
        operation Op::single(
            Read,
            &[
                Object(K::Org),
                Object(K::Project),
                Object(K::Topic),
                Object(K::Thread),
                Object(K::Provider),
                Identity(I::Account),
                Identity(I::System),
            ],
            Target::Resolved("scope"),
        );
    /// Says what each model usable at a node would be counted at now, where that rate comes from, and which have none.
    UsageRateTableGet: GET "/usage/rates", Rule("usage.view on node_id, or browser-session system admin for system=true"), query UsageRateTableGetQuery, none, json RateTable, 200,
        operation Op::variants(&[
            Variant {
                name: "node",
                selector: "node_id is present",
                facets: &[Facet::required(
                    Read,
                    &[
                        Object(K::Org),
                        Object(K::Project),
                        Object(K::Topic),
                        Object(K::Thread),
                    ],
                    Target::Resolved("node"),
                ), Facet::required(Read, &[Object(K::Provider)], Target::ResolvedCollection("rate_providers"))],
            },
            Variant {
                name: "system",
                selector: "system=true",
                facets: &[Facet::required(
                    Manage,
                    &[Identity(I::System)],
                    Target::System,
                )],
            },
        ]);
    /// Reads the rate estimates as loaded, or with `q` the family that matches a model. Answers `null` when no catalogue has been loaded; an existing catalogue with no matching family has an empty `families` list.
    UsageCatalogueGet: GET "/usage/catalogue", User, query UsageCatalogueGetQuery, none, json Option<RateCatalogue>, 200,
        operation Op::single(Read, &[Identity(I::System)], Target::System);
    /// Replaces the rate catalogue in force by a file, by one fetched from the configured address, or by the shipped one. A system admin.
    AdminUsageCatalogueSet: POST "/admin/usage/catalogue", Session, none, json AdminUsageCatalogueSetBody, json RateCatalogue, 200,
        operation Op::returning(Manage, &[Identity(I::System)], Target::System);
    /// Lists everything that bounds work at a node, with current use.
    UsageLimitsGet: GET "/usage/limits", Rule("node.view on the node named by node_id; usage.view for somebody else's share"), query UsageLimitsGetQuery, none, json Limits, 200,
        operation Op::facets(&[Facet::required(
            Read,
            &[
                Object(K::Org),
                Object(K::Project),
                Object(K::Topic),
                Object(K::Thread),
            ],
            Target::Resolved("node"),
        ), CAP_SELECTED, CAP_GROUPS, USAGE_NAMES]);
    /// Lists the caller's own share of every cap counted per person that covers them, anywhere.
    MeLimitsGet: GET "/me/limits", User, none, none, json Vec<CapStatus>, 200,
        operation Op::single(Read, &[Identity(I::Account)], Target::CurrentAccount);
    /// Lists the caps defined on a node, on a provider or on the system, with their use.
    UsageCapList: GET "/usage/caps", Rule("usage.view on the node, provider.view on the provider, or a system admin for the system's"), query UsageCapListQuery, none, json Vec<CapStatus>, 200,
        operation Op::facets(&[Facet::required(
            Read,
            &[
                Object(K::Org),
                Object(K::Project),
                Object(K::Topic),
                Object(K::Thread),
                Object(K::Provider),
                Identity(I::System),
            ],
            Target::Resolved("cap_subjects"),
        ), CAP_SELECTED, CAP_GROUPS, USAGE_NAMES]);
    /// Lists what waits for room in a cap on running turns, in the order it will run.
    UsageCapWaitingList: GET "/usage/caps/{id}/waiting", Rule("as for reading that cap; and whoever it covers"), none, none, json UsageCapWaitingListReply, 200,
        operation Op::facets(&[Facet::required(
            Read,
            &[
                Object(K::Org),
                Object(K::Project),
                Object(K::Topic),
                Object(K::Thread),
                Object(K::Provider),
                Identity(I::System),
            ],
            Target::Resolved("cap_subjects"),
        ), USAGE_NAMES]);
    /// Makes a cap; a cap with the same node, provider, subject, split, models, tiers, harness kinds, meter and period is `409 conflict.exists`.
    UsageCapCreate: POST "/usage/caps", Rule("provider.manage on the provider, usage.manage on the node, or a system admin for the system's; naming a node as well as a provider needs node.view on it"), none, json CapInput, json UsageCapWriteResponse, 201,
        operation Op::receipt(&Op::facets(&[
            Facet::required(
                Manage,
                &[
                    Object(K::Org),
                    Object(K::Project),
                    Object(K::Topic),
                    Object(K::Thread),
                    Object(K::Provider),
                    Identity(I::System),
                ],
                Target::Resolved("cap_subjects"),
            ),
            Facet::required(
                Read,
                &[
                    Object(K::Org),
                    Object(K::Project),
                    Object(K::Topic),
                    Object(K::Thread),
                ],
                Target::ResolvedCollection("named_nodes"),
            ),
            Facet::response_content(
                &[
                    Object(K::Org),
                    Object(K::Project),
                    Object(K::Topic),
                    Object(K::Thread),
                    Object(K::Provider),
                    Identity(I::System),
                ],
                Target::Resolved("cap_subjects"),
            ),
            Facet::response_content(&[Identity(I::Group)], Target::ResolvedCollection("cap_groups")),
            Facet::response_content(&[Object(K::Org),Object(K::Project),Object(K::Topic),Object(K::Thread)],Target::ResolvedCollection("usage_names")),
        ]));
    /// Changes a cap.
    UsageCapUpdate: PATCH "/usage/caps/{id}", Rule("as for making it"), none, json UsageCapUpdateBody, json UsageCapWriteResponse, 200,
        operation Op::receipt(&Op::facets(&[
            Facet::required(
                Manage,
                &[
                    Object(K::Org),
                    Object(K::Project),
                    Object(K::Topic),
                    Object(K::Thread),
                    Object(K::Provider),
                    Identity(I::System),
                ],
                Target::Resolved("cap_subjects"),
            ),
            Facet::response_content(
                &[
                    Object(K::Org),
                    Object(K::Project),
                    Object(K::Topic),
                    Object(K::Thread),
                    Object(K::Provider),
                    Identity(I::System),
                ],
                Target::Resolved("cap_subjects"),
            ),
            Facet::response_content(&[Identity(I::Group)], Target::ResolvedCollection("cap_groups")),
            Facet::response_content(&[Object(K::Org),Object(K::Project),Object(K::Topic),Object(K::Thread)],Target::ResolvedCollection("usage_names")),
        ]));
    /// Removes a cap.
    UsageCapDelete: DELETE "/usage/caps/{id}", Rule("as for making it"), none, none, none, 204,
        operation Op::single(
            Manage,
            &[
                Object(K::Org),
                Object(K::Project),
                Object(K::Topic),
                Object(K::Thread),
                Object(K::Provider),
                Identity(I::System),
            ],
            Target::Resolved("cap_subjects"),
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_absent_token_report_is_distinct_from_a_reported_zero() {
        assert!(RequestTokens::default().is_empty());
        assert!(
            !RequestTokens {
                input_tokens: Some(0),
                ..Default::default()
            }
            .is_empty()
        );
    }

    #[test]
    fn request_option_facts_do_not_claim_request_performance_measurements() {
        let mut timing = RequestTiming {
            wire_protocol: Some("openai-responses".to_owned()),
            requested_service_tier: Some("auto".to_owned()),
            reported_service_tier: UsageMeasurement::Empty,
            ..Default::default()
        };
        assert!(timing.is_unsupported());
        assert!(!timing.is_empty(), "metadata must survive the API envelope");
        assert!(RequestTiming::default().is_empty());
        timing.request_ms = UsageMeasurement::Told { value: 0 };
        assert!(
            !timing.is_unsupported(),
            "a measured zero remains a real measurement"
        );
    }

    #[test]
    fn tier_only_request_facts_survive_the_real_usage_event_envelope() {
        let mut event: UsageEvent = serde_json::from_value(json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a80",
            "at": "2026-11-06T00:00:00Z",
            "origin": "forwarder", "purpose": "turn", "scope": "request", "counted": true,
            "project": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a81", "kind": "project", "name": "Project", "redacted": false },
            "topic": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a82", "kind": "topic", "name": "Topic", "redacted": false },
            "provider": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a83", "name": "Models", "kind": "api", "address": "Team", "placement": "managed", "status": "ready" },
            "model": "model-a", "model_display": "Model A",
            "tokens": { "input_tokens": 10, "output_tokens": 5, "total_tokens": 15 }
        })).unwrap();
        assert!(
            serde_json::to_value(&event)
                .unwrap()
                .get("timing")
                .is_none()
        );
        event.timing = RequestTiming {
            wire_protocol: Some("openai-responses".to_owned()),
            requested_service_tier: Some("auto".to_owned()),
            reported_service_tier: UsageMeasurement::Empty,
            ..Default::default()
        };
        assert!(event.timing.is_unsupported());
        let encoded = serde_json::to_value(&event).unwrap();
        assert_eq!(encoded["timing"]["wire_protocol"], "openai-responses");
        assert_eq!(encoded["timing"]["requested_service_tier"], "auto");
        assert_eq!(
            encoded["timing"]["reported_service_tier"],
            json!({"state":"empty"})
        );
        assert_eq!(encoded["timing"]["outcome"], json!({"state":"unsupported"}));
        assert_eq!(
            encoded["timing"]["request_ms"],
            json!({"state":"unsupported"})
        );
        assert!(encoded["timing"]["reported_tokens"].is_null());
        assert_eq!(
            serde_json::from_value::<UsageEvent>(encoded).unwrap(),
            event
        );
    }

    fn user(n: u8) -> serde_json::Value {
        json!({
            "id": format!("0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a{n:02x}"), "kind": "human", "username": format!("user{n}"),
            "display_name": format!("User {n}"), "avatar": { "text": "U", "color": 3 }
        })
    }

    fn cap() -> serde_json::Value {
        json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a70",
            "node": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a71", "kind": "project", "name": "Billing", "redacted": false },
            "provider": null,
            "subject": { "kind": "role", "role": "developer" },
            "split": "each",
            "models": ["gpt-6*", "@frontier"],
            "tiers": ["fast"],
            "harness_kinds": [],
            "meter": "credits",
            "period": "month",
            "counted_from": null,
            "amount": "200.5",
            "at_limit": "finish_turn",
            "warn_percent": 80,
            "uncounted": "allow",
            "note": "",
            "enabled": true,
            "revision": 3,
            "created_by": user(1),
            "created_at": "2026-10-01T08:00:00.000Z",
            "updated_at": "2026-10-02T08:00:00.000Z",
            "can_edit": true
        })
    }

    #[test]
    fn a_cap_is_read_with_whom_it_covers_and_how_far_it_is_used() {
        let status = json!({
            "cap": cap(),
            "person": user(2),
            "used": "180.25",
            "in_progress": "3",
            "waiting": 0,
            "left_out": { "unrated_tokens": 120000, "unmetered_threads": 1 },
            "state": "warning",
            "period_ends_at": "2026-11-01T00:00:00.000Z",
            "replaced_by": null,
            "can_bypass": false,
            "bypasses": { "count": 2, "last": { "by": user(1), "at": "2026-10-05T10:00:00.000Z" } }
        });
        let read: CapStatus = serde_json::from_value(status.clone()).unwrap();
        assert_eq!(
            read.cap.subject,
            CapSubject::Role {
                role: Role::Developer,
                at: None,
            }
        );
        assert_eq!(read.state, CapState::Warning);
        assert_eq!(serde_json::to_value(&read).unwrap(), status);
    }

    #[test]
    fn a_cap_on_running_turns_is_made_without_a_period() {
        let made: CapInput = serde_json::from_value(json!({
            "provider_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a72",
            "subject": { "kind": "group", "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a73" },
            "split": "shared",
            "meter": "running",
            "amount": "4"
        }))
        .unwrap();
        assert_eq!(made.meter, CapMeter::Running);
        assert_eq!(made.period, None);
        assert!(matches!(made.subject, CapSubjectInput::Group { .. }));
        let unlimited: CapInput = serde_json::from_value(json!({
            "subject": { "kind": "everyone" }, "split": "shared", "meter": "tokens",
            "period": "day", "amount": null
        }))
        .unwrap();
        assert_eq!(unlimited.amount, None);
        assert!(
            serde_json::from_value::<CapInput>(json!({
                "subject": { "kind": "everyone" }, "split": "shared", "meter": "tokens", "amount": "1", "id": "x"
            }))
            .is_err()
        );
    }

    #[test]
    fn a_cap_names_a_group_of_models_and_the_speeds_it_covers() {
        let made: CapInput = serde_json::from_value(json!({
            "provider_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a72",
            "subject": { "kind": "everyone" }, "split": "shared", "meter": "running", "amount": "1",
            "models": ["@frontier"], "tiers": ["fast"]
        }))
        .unwrap();
        assert_eq!(made.models, Some(vec!["@frontier".to_owned()]));
        assert_eq!(made.tiers, Some(vec!["fast".to_owned()]));
        let plain: CapInput = serde_json::from_value(json!({
            "subject": { "kind": "everyone" }, "split": "shared", "meter": "tokens",
            "period": "day", "amount": "1"
        }))
        .unwrap();
        assert_eq!(
            plain.tiers, None,
            "left out: whatever tier a turn runs with"
        );
        let read: UsageCap = serde_json::from_value(cap()).unwrap();
        assert_eq!(read.models, ["gpt-6*", "@frontier"]);
        assert_eq!(read.tiers, ["fast"]);
        let change: UsageCapUpdateBody =
            serde_json::from_value(json!({ "tiers": [], "expected_revision": 3 })).unwrap();
        assert_eq!(
            change.tiers,
            Some(Vec::new()),
            "an empty list takes the limit off the speed"
        );
    }

    #[test]
    fn a_change_to_a_cap_tells_no_limit_from_leaving_the_amount_alone() {
        let unlimited: UsageCapUpdateBody =
            serde_json::from_value(json!({ "amount": null, "expected_revision": 3 })).unwrap();
        let alone: UsageCapUpdateBody =
            serde_json::from_value(json!({ "note": "raised", "expected_revision": 3 })).unwrap();
        assert_eq!(unlimited.amount, Some(None));
        assert_eq!(alone.amount, None);
        assert!(serde_json::from_value::<UsageCapUpdateBody>(json!({ "amount": "1" })).is_err());
    }

    #[test]
    fn a_key_tells_the_person_who_is_nobody_from_a_dimension_it_is_not_about() {
        let key: UsageKey = serde_json::from_value(json!({
            "person": null,
            "model": "gpt-6-sol",
            "model_display": "GPT-6 Sol",
            "agent": { "kind": "codex", "name": "Codex" },
            "environment": "managed"
        }))
        .unwrap();
        assert_eq!(key.person, Some(None));
        assert_eq!(key.org, None);
        assert_eq!(key.environment, Some(Some(UsageEnvironmentKind::Managed)));
        assert_eq!(
            serde_json::to_value(&key).unwrap(),
            json!({
                "person": null, "model": "gpt-6-sol", "model_display": "GPT-6 Sol",
                "agent": { "kind": "codex", "name": "Codex" }, "environment": "managed"
            })
        );
    }

    #[test]
    fn a_catalogue_body_is_a_file_or_a_request_to_fetch_or_to_put_back() {
        let file: AdminUsageCatalogueSetBody = serde_json::from_value(json!({
            "format": 1, "as_of": "2026-10-10", "unit": "USD",
            "families": [{
                "match": "gpt-5.4*", "vendor": "OpenAI", "source": "https://example.com/pricing", "as_of": "2026-10-10",
                "rate": { "input_per_mtok": "2.50", "cached_input_per_mtok": "0.25", "output_per_mtok": "15",
                          "tiers": { "fast": "2" },
                          "above": [{ "context_tokens": 272000, "input_per_mtok": "5", "output_per_mtok": "22.5" }] }
            }]
        }))
        .unwrap();
        assert!(
            matches!(&file, AdminUsageCatalogueSetBody::File(f) if f.families[0].pattern == "gpt-5.4*")
        );
        assert!(matches!(
            serde_json::from_value::<AdminUsageCatalogueSetBody>(json!({ "fetch": true })).unwrap(),
            AdminUsageCatalogueSetBody::Fetch(_)
        ));
        assert!(matches!(
            serde_json::from_value::<AdminUsageCatalogueSetBody>(json!({ "shipped": true }))
                .unwrap(),
            AdminUsageCatalogueSetBody::Shipped(_)
        ));
        assert!(
            serde_json::from_value::<AdminUsageCatalogueSetBody>(
                json!({ "fetch": true, "shipped": true })
            )
            .is_err()
        );
    }
}

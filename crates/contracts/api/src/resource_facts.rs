//! Shared current dependents and reported use of resources, with explicit history coverage.

use orch_access_contract::decisions::ObjectRef;
use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A resource page, optionally narrowed to one model offered by a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FactTarget {
    /// The kind's ordinary permission page; models use their provider's page.
    pub object: ObjectRef,
    /// An exact provider model membership, or null for the whole resource.
    pub model_id: Option<Id>,
}

/// Optional lifecycle choices for a resource patch, applied only when its switch changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourcePauseQuery {
    /// A short one-line note shown to people whose threads wait on the resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// On Enable, run eligible waiting messages; absent or false keeps them held for Resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_waiting: Option<bool>,
}

/// Enable keeps waiting messages held unless the person explicitly asks to run them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceResumeQuery {
    /// Run eligible waiting messages in their existing order and as their existing authors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_waiting: Option<bool>,
}

/// Current work counted once by the dependents service, before names are projected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActivityGroup {
    /// Distinct turns or waiting messages, in this group's own unit.
    pub count: i64,
    /// Distinct threads containing that work.
    pub threads: i64,
    /// One name per visible thread, never one name per message.
    pub visible: Vec<VisibleDependent>,
    /// Other distinct threads whose names remain private.
    pub hidden: i64,
    /// The earliest work in the group, or null when empty.
    pub since: Option<Time>,
}

/// The same live dependencies used to preview an immediate stop and a deliberate resumption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActivityFacts {
    /// Exact active turn dependencies, including immutable segments still using the source.
    pub running: ActivityGroup,
    /// Eligible queued messages; a person's existing Stop remains held.
    pub waiting: ActivityGroup,
    /// The latest turns stopped by this resource's pause, eligible for visible Continue.
    pub stopped: ActivityGroup,
}

/// A pause, resumption or deletion reads the same current facts before its one confirmation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceImpact {
    /// The exact resource or provider model being affected.
    pub target: FactTarget,
    /// Its current authorized name.
    pub name: String,
    /// Current dependents, without performing an unrelated historical-use read.
    pub facts: ResourceFacts,
    /// The exact work that stops or waits.
    pub activity: ActivityFacts,
    /// A previously accepted stop still has native work to finish.
    pub stopping: bool,
    /// Current reader-specific status for visible bound threads with content access.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub thread_displays: std::collections::BTreeMap<Id, crate::threads::ThreadDisplay>,
}

crate::endpoints! {
    /// Reads current consequences using the resource's native view or management authority.
    ResourceImpactRead: POST "/resources/impact", crate::routes::Access::User, none, json FactTarget, json ResourceImpact, 200;
}

/// A stable subject of the shared facts service. An artifact belongs to one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FactSubject {
    /// A resource with its own permission page.
    Resource {
        /// Its identity, independent of its name or current home.
        object: ObjectRef,
    },
    /// An exact opaque node inventory identity, never a path or a native selector.
    Artifact {
        /// The host whose inventory supplies it.
        host_id: Id,
        /// The driver-neutral inventory kind.
        artifact_kind: String,
        /// Its opaque inventory identifier.
        id: String,
    },
    /// An updater-owned server file set, read through protected operator metadata.
    ServerArtifact {
        /// The shared retained kind, such as release or dump.
        artifact_kind: String,
        /// Its exact operator record identity, never a caller-supplied filesystem path.
        id: String,
    },
}

/// Real reported operations. Being offered, attached or inherited is never a use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResourceUseKind {
    /// A real turn, attributed by the consumption ledger or its immutable segment.
    Turn,
    /// One MCP tool call with a frozen stable server identity.
    McpCall,
    /// One explicitly reported invocation of a delivered skill version.
    SkillInvocation,
    /// One logical environment successfully made from a template or on a host.
    EnvironmentMade,
    /// One logical provider successfully made from a provider template.
    ProviderMade,
}

impl ResourceUseKind {
    /// The durable vocabulary used by fact producers and indexed readers.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::McpCall => "mcp_call",
            Self::SkillInvocation => "skill_invocation",
            Self::EnvironmentMade => "environment_made",
            Self::ProviderMade => "provider_made",
        }
    }
}

/// How much of a subject's history is actually recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UseCoverage {
    /// The recorded source covers its whole lifetime.
    Complete,
    /// The retained source covers only a known interval.
    Partial {
        /// The first instant for which the producer guarantees coverage.
        since: Time,
    },
    /// No trustworthy producer has reported this history.
    Unrecorded,
}

/// A figure and its unit. Absent figures never mean zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceUses {
    /// What a use counts.
    pub kind: ResourceUseKind,
    /// Whether a number is complete, partial since a date, or not recorded.
    pub coverage: UseCoverage,
    /// Exact latest committed use, independent of the daily count window.
    pub last_used_at: Option<Time>,
    /// Distinct operation identities over the recorded history.
    pub all_time: Option<i64>,
    /// Distinct identities in the thirty UTC daily buckets ending today.
    pub last_30_days: Option<i64>,
    /// Distinct captured user identities over recorded history.
    pub people: Option<i64>,
    /// Distinct captured user identities over those same thirty daily buckets.
    pub people_30_days: Option<i64>,
}

/// Why another thing depends on this subject. Provenance is not an in-use hold.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DependentRelation {
    /// A thread's present binding.
    Binding,
    /// A scope where a resource is explicitly given, including that scope's descendants.
    ExplicitGiving,
    /// An actual attachment, location or session.
    Attachment,
    /// A native object stands on this artifact, including while stopped.
    NativeDependency,
    /// A frozen recovery promise still retains it.
    Restore,
    /// Accepted work still needs it.
    Operation,
    /// This artifact serves this resource; this link alone does not block cleanup.
    Serves,
}

/// A dependent whose name the reader may see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VisibleDependent {
    /// Its permission-page identity where it has one.
    pub object: Option<ObjectRef>,
    /// Its authorized current name.
    pub name: String,
    /// The meaning of the edge.
    pub relation: DependentRelation,
    /// A recovery promise's frozen deadline, if any.
    pub until: Option<Time>,
}

/// One current relationship, counted by the service before names are projected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DependentGroup {
    /// What this count means; unrelated relationships are never added into an unlabeled total.
    pub relation: DependentRelation,
    /// Exact number of distinct dependencies in this relationship.
    pub count: i64,
    /// Names the reader may see, in the same group.
    pub visible: Vec<VisibleDependent>,
    /// Other dependencies whose names and identifiers remain private.
    pub hidden: i64,
    /// The latest frozen deadline when every dependency has one; null for an indefinite hold.
    #[serde(default)]
    pub until: Option<Time>,
}

/// One privacy projection of the complete facts, shared by plans and people-facing lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceFacts {
    /// The subject whose rights were checked by the caller.
    pub subject: FactSubject,
    /// A single observation instant for the whole batch.
    pub as_of: Time,
    /// Current relationships, each counted once before the same privacy projection.
    pub dependents: Vec<DependentGroup>,
    /// False when a required store is away or cannot report its references.
    pub dependents_complete: bool,
    /// An absent figure means not applicable; an unrecorded one has explicit coverage.
    pub uses: Option<ResourceUses>,
}

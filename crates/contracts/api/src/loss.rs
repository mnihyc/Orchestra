//! Explicit permanent loss and independent recovery plans for environments and their machines.

use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::nodes::NodeRef;
use crate::routes::Access::Rule;

/// The unique affected audience and the confirmation's initial notification choice.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OperationNotificationPlan {
    /// Caller-bound reviewed audience whose visible names can be read on hover or focus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_id: Option<Id>,
    /// Active people counted once for the whole operation.
    pub people: i64,
    /// Initially on when accepting the operation blocks work.
    pub default_on: bool,
}

/// A page of visible recipient names and the count whose names the caller cannot see.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OperationRecipients {
    /// Names visible under the person's current profile audience.
    pub people: crate::common::Page<crate::users::UserRef>,
    /// Other actual recipients, with no identifying data.
    pub hidden: i64,
    /// Actual receivers after their own kind preferences have been applied.
    pub total: i64,
}

/// A permission the administrator will explicitly name themselves in while confirming the operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OperationAdminGrant {
    /// The resource's permission page.
    pub object: orch_access_contract::decisions::ObjectRef,
    /// The permission being granted.
    pub permission: orch_access_contract::permissions::Permission,
    /// The readable resource name.
    pub name: String,
}

/// Manageable environments and a redacted count whose owners must recover them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecoverySources {
    /// Environments the caller may recover themselves, including explicit administrative acts.
    pub environments: Vec<LossResource>,
    /// Environments the caller may not manage.
    pub others: i64,
}

/// The resource whose unreachable files are declared lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LossTarget {
    /// A host and everything on it.
    Host {
        /// The host.
        id: Id,
    },
    /// One environment, without revoking a shared host.
    Environment {
        /// The environment.
        id: Id,
    },
    /// A person's physical machine and its registrations.
    Machine {
        /// The machine.
        id: Id,
    },
}

impl LossTarget {
    /// The resource identifier.
    #[must_use]
    pub const fn id(self) -> Id {
        match self {
            Self::Host { id } | Self::Environment { id } | Self::Machine { id } => id,
        }
    }
    /// The resource kind used by its permission page.
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Host { .. } => "host",
            Self::Environment { .. } => "environment",
            Self::Machine { .. } => "machine",
        }
    }
}

/// A named affected resource, hidden when the reader cannot read its page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LossResource {
    /// Its identifier.
    pub id: Id,
    /// Its visible name, or null.
    pub name: Option<String>,
}

/// One environment in the loss inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LossEnvironment {
    /// The affected environment's safe identity.
    pub environment: LossResource,
    /// Threads the reader may read.
    pub threads: Vec<NodeRef>,
    /// Bound threads whose names the reader cannot read.
    pub threads_hidden: i64,
    /// Measured kept bytes; null means unknown.
    pub kept_bytes: Option<i64>,
}

/// The reviewed consequences of one permanent-loss declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LossPlan {
    /// Explicit self-grants made only when the reviewed act is confirmed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub admin_grants: Vec<OperationAdminGrant>,
    /// Explicit notification choice shown before confirmation.
    pub notification: OperationNotificationPlan,
    /// The expiring plan to confirm.
    pub preflight_id: Id,
    /// When a fresh plan is needed.
    pub expires_at: Time,
    /// What is declared lost.
    pub target: LossTarget,
    /// The exact typed confirmation.
    pub name: String,
    /// Affected environments, including their redacted thread counts.
    pub environments: Vec<LossEnvironment>,
    /// Visible threads elsewhere that depend on a lost provider runtime.
    #[serde(default)]
    pub provider_threads: Vec<NodeRef>,
    /// Other affected provider-bound threads, without names.
    #[serde(default)]
    pub provider_threads_hidden: i64,
    /// Mount locations that are forgotten.
    pub mounts: Vec<LossResource>,
    /// Providers whose runtimes are lost.
    pub providers: Vec<LossResource>,
    /// Jobs that end as lost.
    pub jobs: i64,
    /// Measured retained data, or null when any retained size is unknown.
    pub kept_bytes: Option<i64>,
}

/// Requests a permanent-loss plan without changing anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LossPreflightBody {
    /// The unreachable resource.
    pub target: LossTarget,
}

/// Confirms the unchanged plan by its exact resource name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LossDeclareBody {
    /// Whether to notify the affected people once for this operation.
    #[serde(default)]
    pub notify: bool,
    /// The reviewed plan.
    pub preflight_id: Id,
    /// The name typed by the caller.
    pub confirm: String,
}

/// What happens to one selected environment during recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    /// Carry the answering environment to another host.
    Move,
    /// Make a lost environment again, keeping its identity.
    Recreate,
    /// Switch its threads into the common destination environment.
    SwitchThreads,
}

/// An override for one row of a recovery plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecoveryChoice {
    /// The selected environment.
    pub environment_id: Id,
    /// Its action; absent chooses Move or Re-create from its state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<RecoveryAction>,
    /// An optional destination host for this row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<Id>,
}

/// Plans independent recovery of several environments with one shared destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentRecoverPreflightBody {
    /// Selected environment identities.
    pub environment_ids: Vec<Id>,
    /// Common host; absent means the host with most room.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<Id>,
    /// Optional per-environment overrides.
    #[serde(default)]
    pub choices: Vec<RecoveryChoice>,
    /// Common destination when switching threads, made only once for the whole batch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<crate::forks::ForkEnvironment>,
}

/// The visible plan for one environment in a recovery batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentRecoveryRow {
    /// The environment retaining its identity for a move or re-creation.
    pub environment: crate::environment::EnvironmentRef,
    /// The selected action.
    pub action: RecoveryAction,
    /// The destination host for a re-creation.
    pub host: Option<crate::environment::PlaceHost>,
    /// Cached files that can be materialized for a re-creation.
    pub files: Option<crate::forks::FilesPlan>,
    /// Whether a workspace listing was available to check coverage.
    pub listing_available: bool,
    /// Whether owned workspace or home volumes were restored on the target.
    pub restored: bool,
    /// Measured restored bytes, or null when unmeasured.
    pub restored_bytes: Option<i64>,
    /// An ordinary environment move plan.
    pub move_plan: Option<crate::forks::EnvironmentMovePlan>,
    /// The switch plan, including redacted threads.
    pub switch_plan: Option<crate::forks::ForkBatchPlan>,
    /// Why this member cannot run; other members remain independent.
    pub blockers: Vec<crate::forks::PlanBlocker>,
}

/// One reviewed plan and one execution bar for a recovery batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentRecoveryPlan {
    /// Selected environments that belong to other managers, never acted on by this batch.
    pub others: i64,
    /// Explicit self-grants shown before confirmation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub admin_grants: Vec<OperationAdminGrant>,
    /// Explicit notification choice for the whole batch.
    pub notification: OperationNotificationPlan,
    /// The plan to confirm.
    pub preflight_id: Id,
    /// When to review it again.
    pub expires_at: Time,
    /// Independent environment plans.
    pub rows: Vec<EnvironmentRecoveryRow>,
    /// Members that can be executed.
    pub executable: i64,
}

/// What a host deletion does with its remaining environments.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostEnvironmentCascade {
    /// Recover each caller-managed environment first.
    #[default]
    Recover,
    /// Remove with the existing retained-data window.
    Remove,
}
/// What becomes of mount data on the host.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostMountCascade {
    /// Forget locations and retain their definitions.
    #[default]
    Forget,
    /// Delete owned volume data after its mount name is typed.
    DeleteData,
}
/// What becomes of the host's provider runtimes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostProviderCascade {
    /// Keep the provider unavailable until its managers act.
    #[default]
    Leave,
    /// Recreate each runtime at the selected destination.
    Recreate,
}

/// One real decision for data that would otherwise be left behind by a host deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostDataFate {
    /// Keep this exact data on the departing machine.
    Leave,
    /// Copy it to a new location on another host before releasing its source.
    Move,
    /// Erase owned mount data after its name is typed.
    Delete,
    /// Keep a removed environment's original recovery deadline.
    Wait,
    /// Purge retained environment files after their name is typed.
    Purge,
}

/// A data choice names a reviewed row, never a physical path or native object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostDataChoice {
    /// Opaque identity supplied by the plan.
    pub id: String,
    /// Its selected fate.
    pub fate: HostDataFate,
    /// Destination for Move; the common destination is used when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<Id>,
}

/// Data and its real fates, with hidden names and unmeasured sizes left unknown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HostDataRow {
    /// The opaque selection identity.
    pub id: String,
    /// The resource that owns these files, without its native path.
    pub resource: LossResource,
    /// Native object name when no logical resource owns this data.
    pub label: Option<String>,
    /// Mount or environment data.
    pub kind: String,
    /// Measured size, or null.
    pub bytes: Option<i64>,
    /// Original frozen recovery deadline, or null for mount data.
    pub until: Option<Time>,
    /// The reviewed fate.
    pub selected: HostDataFate,
    /// Fates that this kind can select; a refusal belongs beside the item.
    pub choices: Vec<HostDataFate>,
    /// The selected destination, if any.
    pub host: Option<crate::resources::HostRef>,
    /// Existing mount-copy measurement, when Move is selected.
    pub copy: Option<crate::forks::PlanMountCopy>,
    /// Why the selected fate cannot proceed.
    pub blockers: Vec<crate::forks::PlanBlocker>,
}
/// The choices reviewed before a host deletion.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostDeletePreflightBody {
    /// Environments recover first by default.
    #[serde(default)]
    pub environments: HostEnvironmentCascade,
    /// Mount locations are forgotten by default.
    #[serde(default)]
    pub mounts: HostMountCascade,
    /// Providers are left unavailable by default.
    #[serde(default)]
    pub providers: HostProviderCascade,
    /// The common destination, or the host with most room.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<Id>,
    /// Per-item data fates override the shared mount preset.
    #[serde(default)]
    pub data: Vec<HostDataChoice>,
    /// Erase owned non-data objects after cascades; on when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub erase: Option<bool>,
}
/// One compact kind of dependency with its current blockers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostCascadeRow {
    /// Environments, mount_locations, mcp_servers or provider_runtimes.
    pub kind: String,
    /// The number on the source host.
    pub count: i64,
    /// Why this kind cannot yet be cleared.
    pub blockers: Vec<crate::forks::PlanBlocker>,
}
/// One reviewed deletion, with independent recovery work before physical cleanup and revocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HostDeletePlan {
    /// An already accepted deletion opens its existing progress instead of accepting another.
    pub job: Option<crate::workspace::Job>,
    /// Explicit notification choice and recipient preview.
    pub notification: OperationNotificationPlan,
    /// Explicit administrative page additions.
    pub admin_grants: Vec<OperationAdminGrant>,
    /// The expiring reviewed plan.
    pub preflight_id: Id,
    /// When the plan expires.
    pub expires_at: Time,
    /// Exact host name required at confirmation.
    pub name: String,
    /// Whether the host answers now.
    pub online: bool,
    /// Remaining dependencies, counts first.
    pub rows: Vec<HostCascadeRow>,
    /// The caller's reviewed environment recoveries.
    pub recovery: Vec<EnvironmentRecoveryRow>,
    /// Environments only their own managers can recover or remove.
    pub others: i64,
    /// Volume mounts whose data deletion needs their exact names.
    pub volumes: Vec<LossResource>,
    /// Latest existing retained-data deadline; it is never shortened by deleting a host.
    pub kept_until: Option<Time>,
    /// Each data item keeps its own fate and deadline.
    pub data: Vec<HostDataRow>,
    /// Owned non-data objects offered for erasure, with their measured sizes.
    pub objects: Vec<crate::storage::KeptRecord>,
    /// Whether the reviewed plan erases those non-data objects.
    pub erase: bool,
    /// Why the selected physical erasure cannot yet be reviewed.
    pub erase_blockers: Vec<crate::forks::PlanBlocker>,
    /// The ordinary background inventory read followed by the plan.
    pub inventory_measurement: Option<crate::forks::Measurement>,
    /// Whether all selected cascades can run.
    pub executable: bool,
}
/// One typed consent for a volume mount's data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostMountConfirmation {
    /// The mount whose volume is deleted.
    pub mount_id: Id,
    /// Its exact name.
    pub confirm: String,
}

/// Exact-name consent for one reviewed data row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostDataConfirmation {
    /// The plan's opaque data identity.
    pub id: String,
    /// The exact owner or native-object name shown in that row.
    pub confirm: String,
}
/// Confirms the unchanged cascade and each selected owned volume deletion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostDeleteCreateBody {
    /// The reviewed cascade.
    pub preflight_id: Id,
    /// The host's exact name.
    pub confirm: String,
    /// Explicit notification choice for the whole operation.
    #[serde(default)]
    pub notify: bool,
    /// Typed volume-mount names, only when Delete data was selected.
    #[serde(default)]
    pub volumes: Vec<HostMountConfirmation>,
    /// Consent for each selected Delete or Purge now row.
    #[serde(default)]
    pub data_confirmations: Vec<HostDataConfirmation>,
}

crate::endpoints! {
    /// Plans the selected cascades before removing a host.
    HostDeletePreflight: POST "/hosts/{id}/delete/preflight", Rule("the host's managers"), none, json HostDeletePreflightBody, json HostDeletePlan, 200;
    /// Runs the reviewed cascades and finally revokes and removes the host registration.
    HostDeleteCreate: POST "/hosts/{id}/delete", Rule("the host's managers"), none, json HostDeleteCreateBody, json crate::workspace::Job, 202;
    /// Names visible to the caller in their own unexpired operation preview.
    OperationRecipientsGet: GET "/operations/{id}/recipients", Rule("the operation preview's creator"), query crate::common::PageQuery, none, json OperationRecipients, 200;
    /// Lists everything affected by declaring an unreachable resource lost.
    LossPreflight: POST "/loss/preflight", Rule("host managers, environment managers, or the machine owner"), none, json LossPreflightBody, json LossPlan, 200;
    /// Declares the reviewed resource permanently lost without contacting its machine.
    LossDeclare: POST "/loss", Rule("host managers, environment managers, or the machine owner"), none, json LossDeclareBody, json LossPlan, 200;
    /// Lists the caller's manageable environments on a living or lost resource.
    LossRecoveryList: POST "/loss/recovery", Rule("host managers, environment managers, or the machine owner"), none, json LossPreflightBody, json RecoverySources, 200;
    /// Plans independent moves, re-creations and switches of selected environments.
    EnvironmentRecoverPreflight: POST "/environments/recover/preflight", Rule("managers of each selected environment"), none, json EnvironmentRecoverPreflightBody, json EnvironmentRecoveryPlan, 200;
    /// Executes the reviewed independent recovery plans under the common batch limit.
    EnvironmentRecoverCreate: POST "/environments/recover", Rule("managers of each selected environment"), none, json crate::forks::ForkCreateBody, json crate::environment::EnvironmentBatch, 202;
    /// Reads the currently visible members of an independent environment batch.
    EnvironmentRecoverGet: GET "/environments/recover/{batch_id}", Rule("the batch creator or a reader of an affected environment"), none, none, json crate::environment::EnvironmentBatch, 200;
}

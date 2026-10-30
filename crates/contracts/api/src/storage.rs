//! One retained-storage report and reviewed cleanup plans across resources and machines.

use orch_access_contract::decisions::ObjectRef;
use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::PageQuery;
use crate::nodes::NodeRef;
use crate::resources::{HostRef, MachineRef};
use crate::routes::Access::{Rule, User};
use crate::users::UserRef;

/// What is kept, independent of where its bytes are stored.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    /// A place in Recently deleted.
    DeletedPlace,
    /// Files retained after an environment was removed.
    EnvironmentFiles,
    /// A persistent owned volume.
    Volume,
    /// Data displaced from a mount location.
    MountData,
    /// Installed files retained from an earlier unit.
    KeptLayer,
    /// One immutable skill archive.
    SkillVersion,
    /// An execution or helper unit.
    Unit,
    /// An owned network.
    Network,
    /// A cached image.
    Image,
    /// A build result or step.
    Build,
    /// A verified tool version.
    ToolVersion,
    /// The active database.
    Database,
    /// Content in the server's blob store.
    Content,
    /// An unfinished browser upload.
    Upload,
    /// A completed export archive.
    Export,
    /// An updater payload release.
    Release,
    /// A database dump with its manifest and paired content copy.
    Dump,
}

/// The reason an object remains, shown consistently on every storage screen.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum KeptWhy {
    /// Current work or another retained object needs it.
    InUse,
    /// A promised recovery window has not ended, or recovery data is kept indefinitely.
    Restore,
    /// Reusable content whose age counts from its last known use.
    Cache,
    /// Ownership or use cannot be established completely.
    Unknown,
}

/// Where cleanup happens; an away machine is never reported as cleaned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StorageAvailability {
    /// The store is reachable.
    Ready,
    /// Its machine is not connected.
    Away,
    /// Only a historical ownership record remains.
    Unknown,
}

/// One item, derived from its existing owner record or a node's observed inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeptRecord {
    /// A stable opaque key; cleanup re-reads it rather than trusting supplied paths.
    pub id: String,
    /// The kind of thing kept.
    pub kind: StorageKind,
    /// Its readable name.
    pub name: String,
    /// The resource to which it belongs, where one still exists.
    pub resource: Option<ObjectRef>,
    /// Its place, where the caller may read that place.
    pub place: Option<NodeRef>,
    /// Its host, where the caller may manage that host.
    pub host: Option<HostRef>,
    /// Its connected machine, where applicable.
    pub machine: Option<MachineRef>,
    /// Its known owner; unknown never invents a person.
    pub owner: Option<UserRef>,
    /// Measured bytes, never zero for unknown.
    pub bytes: Option<i64>,
    /// An opaque physical-content key for counting shared bytes only once.
    pub accounting_key: String,
    /// A larger record already includes these bytes in this report.
    pub included_in: Option<String>,
    /// Why it stays.
    pub why: KeptWhy,
    /// When this retention or observation began.
    pub since: Option<Time>,
    /// Its existing promised end or automatic cache eligibility date; null means no known end.
    pub until: Option<Time>,
    /// The shared dependency projection; absence means this source is not yet recorded.
    pub facts: Option<crate::resource_facts::ResourceFacts>,
    /// When these facts were observed.
    pub observed_at: Time,
    /// Whether its store can act now.
    pub availability: StorageAvailability,
    /// Whether the caller may include it in a cleanup plan.
    pub can_clean: bool,
    /// A stable reason that prevents removal, shown as an item rather than a request conflict.
    pub blocker: Option<String>,
    /// The exact local updater command for operator-owned files; absent rows use the ordinary job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator_command: Option<String>,
}

/// One uncounted reverse reference from the protected operator source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OperatorStorageDependency {
    /// A stable source membership, such as server/current.
    pub key: String,
    /// Its operator-visible name.
    pub name: String,
    /// The shared relationship vocabulary.
    pub relation: crate::resource_facts::DependentRelation,
}

/// One operator record uses the common retained fields and carries references before counting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OperatorStorageItem {
    /// The same retained record the report and plan show.
    pub record: KeptRecord,
    /// Current/previous links, transaction holds or explicit paired-copy references.
    pub dependencies: Vec<OperatorStorageDependency>,
}

/// A protected read-only snapshot published where the updater owns its files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OperatorStorageInventory {
    /// The first supported metadata format.
    pub format_version: u32,
    /// The installation's public authority fingerprint.
    pub system_id: String,
    /// When the updater measured these files and links.
    pub observed_at: Time,
    /// Every known release and dump pair, including protected or unrecorded items.
    pub items: Vec<OperatorStorageItem>,
    /// Sources that could not be read; a partial snapshot never claims they are empty.
    #[serde(default)]
    pub issues: Vec<String>,
}

/// A report's totals count measured content once and state how many sizes remain unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StorageTotals {
    /// Number of retained records.
    pub items: i64,
    /// Known bytes, without substituting zero for unmeasured records.
    pub bytes: i64,
    /// Number of unmeasured physical records.
    pub unknown: i64,
}

/// One way to divide the same report; pressing a row filters the entire screen.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum StorageDimension {
    /// Physical host.
    Host,
    /// Kind of retained object.
    Kind,
    /// Place of ownership.
    Place,
    /// Person owning it.
    Owner,
    /// Age since use or retention began.
    Age,
    /// Why it stays.
    Why,
}

/// A named member of one report dimension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StorageBreakdownRow {
    /// The filter value, opaque unless it is an existing identifier.
    pub key: String,
    /// The readable name, or a stable translated-value key for kinds and age groups.
    pub name: String,
    /// A permitted host or place identity, so its chip needs no further row read.
    pub object: Option<ObjectRef>,
    /// A person, rendered with the shared person chip.
    pub owner: Option<UserRef>,
    /// Figures of this same visible subset.
    pub totals: StorageTotals,
}

/// A table in the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StorageBreakdown {
    /// What divides it.
    pub by: StorageDimension,
    /// Largest measured members first.
    pub rows: Vec<StorageBreakdownRow>,
}

/// Scope and filters of the single storage report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StorageQuery {
    /// `me`, `node:<id>`, `host:<id>` or `system`; defaults to the caller's own storage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// A resource menu narrows the same report, rather than opening another inventory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_kind: Option<orch_access_contract::permissions::ObjectKind>,
    /// The selected resource's identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<Id>,
    /// Host breakdown key: an identifier, server or unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Place breakdown key: an identifier or unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    /// Owner breakdown key: an identifier or unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Retained-object kind filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<StorageKind>,
    /// Retention-reason filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<KeptWhy>,
    /// `under_7_days`, `7_to_30_days`, `30_days_plus` or `unknown`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age: Option<String>,
    /// The existing list convention.
    #[serde(flatten)]
    pub page: PageQuery,
}

/// The same screen's figures, breakdowns and page of retained records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StorageReport {
    /// The resolved door of the report.
    pub scope: String,
    /// The readable scope name.
    pub name: String,
    /// This report's observation time.
    pub at: Time,
    /// Totals of all matching records the caller may manage.
    pub totals: StorageTotals,
    /// Breakdowns of those same records.
    pub breakdowns: Vec<StorageBreakdown>,
    /// One page of retained items.
    pub items: Vec<KeptRecord>,
    /// Opaque continuation, or null at the end.
    pub next_cursor: Option<String>,
    /// Stores whose inventory is not available; an empty record list never claims these are empty.
    pub unavailable: Vec<StorageSourceIssue>,
}

/// One store whose contents or fresh inventory cannot be established.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StorageSourceIssue {
    /// Its authorized host reference, when it belongs to a host.
    pub host: Option<HostRef>,
    /// A stable translated reason, never a guessed empty inventory.
    pub reason: String,
}

/// One fate in a destructive operation, from the existing retention defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CleanupChoice {
    /// Keep it with its current retention promise.
    Keep,
    /// End the resource but keep its recoverable data.
    Remove,
    /// Erase the selected owned data.
    Purge,
    /// Stop an owned unit and remove it, preserving volumes unless selected separately.
    StopAndRemove,
}

/// A cleanup item's displayed record and the permitted choices for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CleanupItem {
    /// The common kept-until record.
    pub record: KeptRecord,
    /// Permitted fates, always including Keep.
    pub choices: Vec<CleanupChoice>,
    /// The registry's default fate.
    pub selected: CleanupChoice,
}

/// The request that scopes a resource cleanup or one selection from the Storage report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CleanupRequest {
    /// The same report door.
    pub scope: String,
    /// A resource menu's object, or null for a record selection.
    pub resource: Option<ObjectRef>,
    /// Selected opaque record keys; empty means this resource's cleanup candidates.
    pub items: Vec<String>,
}

/// The shared destructive preview, stored in the existing expiring change-plan store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CleanupPlan {
    /// The existing plan identifier.
    pub id: Id,
    /// When this observed plan must be read again.
    pub expires_at: Time,
    /// The scope's name typed once before owned data is destroyed.
    pub confirmation_name: String,
    /// Every fate and blocker, including items that cannot go ahead.
    pub items: Vec<CleanupItem>,
    /// People visible to the caller who would receive the accepted operation's notice.
    pub people: Vec<UserRef>,
    /// Additional eligible recipients whose names stay private.
    pub other_people: i64,
    /// The resolved operation-notice default.
    pub notify: bool,
}

/// One edited fate in the accepted plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CleanupSelection {
    /// An exact key from that plan.
    pub id: String,
    /// A permitted fate for this item.
    pub choice: CleanupChoice,
}

/// The one confirmation, backed by the stored observations and current permissions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CleanupStart {
    /// The preview that was actually read.
    pub plan_id: Id,
    /// Fates edited by the person; absent rows use the displayed defaults.
    pub choices: Vec<CleanupSelection>,
    /// Required only when selected data is destroyed.
    pub confirmation_name: Option<String>,
    /// One choice for the whole operation, then narrowed by each recipient's preferences.
    pub notify: bool,
}

/// One item's factual outcome, retained on the common job even after partial failure or cancellation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CleanupOutcome {
    /// The displayed item key.
    pub id: String,
    /// `removed`, `kept`, `blocked` or `failed`.
    pub outcome: String,
    /// Safe failure or retention reason where relevant.
    pub reason: Option<String>,
}

crate::endpoints! {
    /// Reads retained storage the caller may manage through one scoped report.
    StorageGet: GET "/storage", Rule("only storage the caller may manage in this scope"), query StorageQuery, none, json StorageReport, 200;
    /// Previews the fates and blockers of selected retained storage.
    StorageCleanupPreflight: POST "/storage/cleanup/preflight", User, none, json CleanupRequest, json CleanupPlan, 200;
    /// Starts one confirmed cleanup with current authority and the preview's exact objects.
    StorageCleanupCreate: POST "/storage/cleanup", User, none, json CleanupStart, json crate::workspace::Job, 202;
}

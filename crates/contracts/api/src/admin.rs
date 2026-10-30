//! Management views: users and their state with what a new person's own org starts as and which proofs are closed at their name, a reset link that may switch a second step off, root orgs and their two limits, both pools of runtimes, health, stored metric series and PKI.
//!
//! Management requests need a system admin in a browser session: a request made with an API token
//! is refused with `auth.session_required`. Shared program metadata and scoped version fetches use
//! their declared user, environment and settings authority. Inside an org a system admin uses the ordinary
//! requests, as the Admin of every org that they are. A [`Runtime`] is a
//! unit that a host runs, an [`AdminUser`] a person with what only a system
//! admin reads about them, and an [`IdentityProvider`] a single sign-in
//! method. [`HarnessBuild`]s say which builds of a harness may be fetched.
//! [`SemanticSearchOverview`] is search by meaning across the system: each
//! root org's index with its progress, estimate, disk and reason.

use std::collections::BTreeMap;

use orch_types::{Id, Json, Revision, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::{PageChange, PagePreview, PermissionPage, Role};
use crate::auth::SignInProof;
use crate::common::{Page, PageQuery};
use crate::documents::DeletedFilter;
use crate::environment::{EnvironmentRef, NetworkState, TemplateRef};
use crate::nodes::OrgType;
use crate::nodes::{AuditEntry, NodeRef, Org};
use crate::resources::{
    Agent, CapabilityDef, Host, MachineOs, MachineRef, OrgAgentCreateBody, ProviderRef, UnitKind,
};
use crate::routes::Access::{Permission, Public, Rule, Session, User};
use crate::settings::{ResolvedSetting, SettingReach};
use crate::users::UserRef;

/// The most days an export of the audit trail may span.
pub const AUDIT_EXPORT_MAX_DAYS: u32 = 366;
/// The most points a stored series returns for one name.
pub const METRIC_MAX_POINTS: u32 = 720;
/// The widths of a point of a series, in minutes: a minute, five minutes, an hour and a day.
pub const METRIC_STEP_MINUTES: [u32; 4] = [1, 5, 60, 24 * 60];
/// What the key of an identity provider looks like: it appears in callback addresses.
pub const IDENTITY_PROVIDER_KEY_PATTERN: &str = "^[a-z][a-z0-9-]{1,31}$";

/// Whether a person may sign in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdminUserStatus {
    /// They may.
    Active,
    /// Their sign-in is ended everywhere: every session and token of theirs is refused at once, and
    /// what they own stays theirs.
    Suspended,
}

/// A proof that strangers' wrong tries have locked at a name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminUserLock {
    /// The proof.
    pub way: SignInProof,
    /// Until when it is locked.
    pub until: Time,
}

/// A person with what only a system admin reads about them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminUser {
    /// What names them wherever they are pointed at.
    #[serde(flatten)]
    pub base: UserRef,
    /// Their address.
    pub email: Option<String>,
    /// Whether they confirmed it.
    pub email_verified: bool,
    /// Whether they may sign in.
    pub status: AdminUserStatus,
    /// Whether they are a system admin.
    pub is_system_admin: bool,
    /// Whether they set up an authenticator as a second step.
    pub totp_enabled: bool,
    /// Whether they chose a mailed code as a second step.
    pub email_code_second_step: bool,
    /// The proofs that strangers' wrong tries have locked at this name for now: what a person who
    /// "cannot sign in" ran into.
    pub locked: Vec<AdminUserLock>,
    /// When a system admin gave them an invitation to make one team org that they have not used;
    /// `null` when they hold none.
    #[serde(default)]
    pub org_invited_at: Option<Time>,
    /// When they were made.
    pub created_at: Time,
    /// When they were last seen.
    pub last_seen_at: Option<Time>,
}

/// A unit that a host runs, whoever's it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Runtime {
    /// The runtime's identifier.
    pub id: Id,
    /// Whether it is an environment or a provider's runtime.
    pub kind: UnitKind,
    /// The host it is on.
    pub host_id: Id,
    /// The driver that made it.
    pub driver: String,
    /// The driver's name for the unit.
    pub unit_name: String,
    /// Where it stands.
    pub state: String,
    /// A sentence on that, where there is one.
    pub state_detail: Option<String>,
    /// How many units it has had.
    pub generation: i64,
    /// The machine it runs on.
    pub machine: Option<MachineRef>,
    /// Its network: the policy in force, as its host's node last reported it.
    pub network: NetworkState,
    /// The template of its own kind that it was made from.
    pub template: TemplateRef,
    /// How many things it is behind in.
    pub updates: i64,
    /// For a candidate being prepared to replace a runtime, that runtime.
    pub successor_of: Option<Id>,
    /// For a retired one, when the reaper may remove it.
    pub remove_at: Option<Time>,
    /// The environment, for an environment's runtime.
    pub environment: Option<EnvironmentRef>,
    /// The provider, for a provider's runtime.
    pub provider: Option<ProviderRef>,
    /// When something last happened in it.
    pub last_active_at: Option<Time>,
}

/// Which stored values a series was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MetricResolution {
    /// The values of each minute.
    Minute,
    /// The values of each hour.
    Hour,
}

/// One series of what the system stored about itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MetricSeries {
    /// The metric's name.
    pub name: String,
    /// Its labels.
    pub labels: BTreeMap<String, String>,
    /// Which stored values it was read from.
    pub resolution: MetricResolution,
    /// One point for each step: a gauge's mean, or a counter's sum.
    pub points: Vec<(Time, f64)>,
    /// From hours: the largest minute value in each step.
    pub peaks: Option<Vec<(Time, f64)>>,
}

/// What the blob store holds of one environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StorageEnvironment {
    /// The environment.
    pub environment: EnvironmentRef,
    /// The bytes of cached blobs it holds.
    pub cache_bytes: i64,
    /// The bytes of pinned blobs it holds.
    pub pinned_bytes: i64,
}

/// What the blob store holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StorageStats {
    /// How many blobs.
    pub blob_count: i64,
    /// Their size in bytes.
    pub blob_bytes: i64,
    /// The bytes of pinned blobs, which are kept.
    pub pinned_bytes: i64,
    /// The bytes of cached blobs, which may be evicted.
    pub cache_bytes: i64,
    /// The bytes free on the disk.
    pub disk_free_bytes: i64,
    /// The environments that hold the most.
    pub top_environments: Vec<StorageEnvironment>,
}

/// The certificate authority of the system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PkiCa {
    /// Its subject.
    pub subject: String,
    /// From when it is valid.
    pub not_before: Time,
    /// Until when it is valid.
    pub not_after: Time,
    /// The SHA-256 fingerprint of its certificate, in lower case hex.
    pub fingerprint_sha256: String,
}

/// The certificate of one service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PkiService {
    /// The service.
    pub name: String,
    /// Until when its certificate is valid.
    pub not_after: Time,
}

/// A machine whose certificate needs attention, with the time it ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PkiInfoMachinesExpiring {
    /// The machine, whose page offers certificate renewal.
    pub machine: MachineRef,
    /// Until when its certificate is valid.
    pub not_after: Time,
}

/// The state of the system's certificates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PkiInfo {
    /// The authority.
    pub ca: PkiCa,
    /// The services' certificates.
    pub services: Vec<PkiService>,
    /// How many machines' certificates expire within 30 days.
    pub machines_expiring_30d: i64,
    /// At most 50 active machines whose certificates end within 30 days, including those already
    /// lapsed, soonest first. The count above includes machines beyond this list.
    pub machines_expiring: Vec<PkiInfoMachinesExpiring>,
}

/// The parts of the system that report their own health.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SystemComponentName {
    /// The API.
    Api,
    /// The gateway.
    Gateway,
    /// The worker.
    Worker,
    /// The database.
    Database,
}

/// How one part of the system fares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SystemComponent {
    /// Which part.
    pub name: SystemComponentName,
    /// Whether it is well.
    pub ok: bool,
    /// What is wrong, where something is.
    pub detail: Option<String>,
    /// When it last reported.
    pub seen_at: Option<Time>,
}

/// How much waits in the system's queues.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SystemQueues {
    /// Deliveries waiting in the outbox.
    pub outbox_pending: i64,
    /// Commands to machines not finished.
    pub commands_open: i64,
    /// Jobs running.
    pub jobs_running: i64,
}

/// The state of the system: its release, its parts and its queues.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SystemInfo {
    /// The release.
    pub version: String,
    /// The build.
    pub build: String,
    /// When the API started.
    pub started_at: Time,
    /// The parts and how they fare.
    pub components: Vec<SystemComponent>,
    /// How many machines are connected.
    pub machines_online: i64,
    /// How many threads are running.
    pub threads_running: i64,
    /// The queues.
    pub queues: SystemQueues,
}

/// A read-only lookup at the operator's configured release source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReleaseCheck {
    /// Release notes at the operator's source; a link only, never a host update action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes_url: Option<String>,
    /// Time of the last successful lookup; the first failed lookup has its attempt time here.
    pub checked_at: Time,
    /// The release currently installed.
    pub installed_version: String,
    /// Last successfully validated offered version, if one was read.
    pub offered_version: Option<String>,
    /// Whether that offer is newer than the installed release; unknown before a valid answer.
    pub newer: Option<bool>,
    /// The operator's release source, defaulting to this repository's GitHub releases.
    pub source: String,
    /// Safe latest failure, alongside retained successful facts.
    pub error: Option<String>,
    /// Operator command as text only; never executed by the API or worker.
    pub command: Option<String>,
}

/// A failed local backup attempt, separate from the last successful completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BackupFailure {
    /// The failed attempt's time.
    pub at: Time,
    /// Safe local reason without credentials or dump contents.
    pub reason: String,
}

/// Why the existing command took this database-only backup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackupCause {
    /// An operator ran backup.
    Manual,
    /// The updater saved the database before changing application payloads.
    PreUpdate,
}

/// The completed dump's metadata, reconciled from protected local installation state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DatabaseBackupRecord {
    /// When publication of the verified dump pair completed.
    pub completed_at: Time,
    /// The database snapshot's point.
    pub database_at: Time,
    /// Installed release at that point.
    pub release: String,
    /// Applied schema at that point.
    pub schema: String,
    /// Relative dump name, never an arbitrary local path.
    pub dump_name: String,
    /// Dump size in bytes.
    pub bytes: u64,
    /// The dump's BLAKE3 content hash.
    pub hash: String,
    /// Why it was taken.
    pub cause: BackupCause,
    /// Always true: blobs, keys, configuration and execution volumes are excluded.
    pub database_only: bool,
    /// A later failed attempt never advances completed_at.
    pub last_failure: Option<BackupFailure>,
}

/// One location recorded by the backup writer; it is not evidence of a separate host file backup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BackupInventoryLocation {
    /// Environment, provider or mount identifier, where that inventory has one.
    pub object_id: Option<Id>,
    /// Host which held it at the snapshot, where reported.
    pub host_id: Option<Id>,
    /// Connected machine which held it at the snapshot, where reported.
    pub machine_id: Option<Id>,
    /// Runtime containing a private overlay, where reported.
    pub runtime_id: Option<Id>,
    /// Driver recorded at the snapshot.
    pub driver: Option<String>,
    /// Named host storage, where reported.
    pub storage: Option<String>,
    /// Private-overlay mount name, where reported.
    pub mount: Option<String>,
    /// Recorded path or volume name; no file content or credentials.
    pub location: Option<String>,
    /// Whether the writer reported this location as known.
    pub location_known: bool,
    /// Availability of a separate backup copy; null because the database-only writer did not inspect one.
    pub copy_available: Option<bool>,
}

/// One category of the verified manifest's restore inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BackupInventoryGroup {
    /// Stable category from the manifest, such as database, workspaces or browser_profiles.
    pub category: String,
    /// Actual count at the snapshot, or null when the writer could not enumerate it.
    pub count: Option<u64>,
    /// Number of environments requiring inspection, where the manifest records that separately.
    pub environments_requiring_inspection: Option<u64>,
    /// The writer's reason for inclusion or separate backup.
    pub reason: String,
    /// Locations recorded in this category, without pretending an absent list means zero items.
    pub locations: Vec<BackupInventoryLocation>,
}

/// A restore list from the verified manifest paired with this exact last successful backup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BackupRestoreList {
    /// Completion, database point and dump identity the list belongs to.
    pub backup: DatabaseBackupRecord,
    /// Actual immutable manifest copy's plain name in protected installation state.
    pub manifest_name: String,
    /// BLAKE3 of that exact manifest copy.
    pub manifest_hash: String,
    /// What the database-only pair contains.
    pub included: Vec<BackupInventoryGroup>,
    /// What requires separate copies or inspection; availability is not inferred from a location.
    pub excluded: Vec<BackupInventoryGroup>,
}

/// Installation facts; neither a timestamp-posting entrance nor a backup scheduler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstallationRecords {
    /// Last release check, if one was requested or enabled.
    pub release: Option<ReleaseCheck>,
    /// Last verified database-only backup, if one is known.
    pub backup: Option<DatabaseBackupRecord>,
    /// No successful backup, or its age exceeds backup.warn_days.
    pub backup_warning: bool,
    /// An unreadable, mismatching or failed local record, kept separate from success.
    pub backup_error: Option<String>,
}

/// How an identity provider is made or changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdentityProviderInput {
    /// Its key, which appears in callback addresses; it matches [`IDENTITY_PROVIDER_KEY_PATTERN`].
    pub key: String,
    /// The label of its button.
    pub name: String,
    /// The issuer, whose discovery is at `<issuer>/.well-known/openid-configuration`.
    pub issuer: String,
    /// The client identifier; the secret is kept sealed.
    pub client_id: String,
    /// The client secret; write-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// The scopes asked for; `openid`, `profile` and `email` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    /// The claim that names the person; `sub` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_subject: Option<String>,
    /// The claim that holds their address; `email` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_email: Option<String>,
    /// The claim that holds their name; `name` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_name: Option<String>,
    /// The claim that holds their username; `preferred_username` unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_username: Option<String>,
    /// The domains of addresses let in; empty for any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_email_domains: Option<Vec<String>>,
    /// Whether a user is made at the first sign-in, still subject to the registration mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_register: Option<bool>,
    /// The org joined at auto-registration, at any level: the person's domain there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_org_id: Option<Id>,
    /// Their role in that org.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_role: Option<Role>,
    /// Whether it is offered; on unless given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// A single sign-in method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IdentityProvider {
    /// Its identifier.
    pub id: Id,
    /// Its key, which appears in callback addresses.
    pub key: String,
    /// The label of its button.
    pub name: String,
    /// The issuer.
    pub issuer: String,
    /// The client identifier.
    pub client_id: String,
    /// The scopes asked for.
    pub scopes: Vec<String>,
    /// The claim that names the person.
    pub claim_subject: String,
    /// The claim that holds their address.
    pub claim_email: String,
    /// The claim that holds their name.
    pub claim_name: String,
    /// The claim that holds their username.
    pub claim_username: String,
    /// The domains of addresses let in; empty for any.
    pub allowed_email_domains: Vec<String>,
    /// Whether a user is made at the first sign-in.
    pub auto_register: bool,
    /// The org joined at auto-registration.
    pub default_org_id: Option<Id>,
    /// Their role in that org.
    pub default_role: Option<Role>,
    /// Whether it is offered.
    pub enabled: bool,
    /// Whether a client secret is set; it is never returned.
    pub client_secret_set: bool,
    /// The address the provider is told to call back.
    pub callback_url: String,
    /// The revision, which a change names as `expected_revision`.
    pub revision: Revision,
}

/// The processor a harness build is made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum HarnessArch {
    /// Intel and AMD, 64 bit.
    #[serde(rename = "x86_64")]
    X86_64,
    /// Arm, 64 bit.
    #[serde(rename = "aarch64")]
    Aarch64,
}

/// Where a harness build came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarnessBuildSource {
    /// The release manifest lists it.
    Release,
    /// A system admin added it.
    Admin,
    /// Orch read it from the vendor's release channel, with the vendor's checksum.
    Channel,
    /// A mirror supplied its checksum under the system administrator’s explicit trust setting.
    Mirror,
}

/// A carried-in list of vendor checksums, imported explicitly by a system administrator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HarnessChecksumsImportBody {
    /// The exact UTF-8 JSON list gathered on the connected side; its SHA-256 is recorded in the audit.
    pub document: String,
}

/// The digest and size of the checksum list accepted by the system administrator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessChecksumsImportReply {
    /// The SHA-256 of the exact imported document.
    pub sha256: String,
    /// How many builds were imported.
    pub count: i64,
}

/// How far a build is known to work with its adapter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarnessBuildStatus {
    /// Nothing checked it yet: it is offered as untested.
    #[default]
    Untested,
    /// Its check runs.
    Checking,
    /// Its interface and native cases passed, or it is the build this release was made with.
    Checked,
    /// Its check found something the adapter uses missing, or could not run; see its check.
    Failed,
}

/// What the check of a build found: once per build, in the background, never when a session starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessBuildCheck {
    /// When it ran.
    pub at: Time,
    /// The machine that ran it, by name.
    pub machine: Option<String>,
    /// What the adapter uses that the build does not have: protocol methods, types or command-line flags, by name.
    pub missing: Vec<String>,
    /// Features switched off because an interface requirement is missing or a native case failed.
    pub off: Vec<String>,
    /// Why it could not run, when it could not.
    pub error: Option<String>,
    /// The current stage, or complete once both stages have ended.
    #[serde(default)]
    pub stage: HarnessCheckStage,
    /// Native cases observed against the scripted model, in execution order.
    #[serde(default)]
    pub cases: Vec<HarnessConformanceCase>,
}

/// The two stages of checking an immutable agent program build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarnessCheckStage {
    /// Checking the interface needed by the adapter.
    #[default]
    Interface,
    /// Running native behavior against the scripted model.
    Conformance,
    /// The check ended, including a failure that prevented the next stage.
    Complete,
}

/// One native case and the feature affected by its failure on this build alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessConformanceCase {
    /// Stable case name, also shown when it fails.
    pub name: String,
    /// Whether the native behavior was observed and satisfied the case.
    pub passed: bool,
    /// Descriptor feature disabled on failure or proved unavailability; null without an individual switch.
    pub feature: Option<String>,
    /// Why an optional feature was proved unavailable while its refusal check passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsupported: Option<String>,
    /// Why the case failed; null on success.
    pub error: Option<String>,
    /// Time spent running the case, in milliseconds.
    pub elapsed_ms: i64,
}

/// Why release channels are or are not read on a schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarnessChannelSchedule {
    /// Every place uses shipped or exact builds.
    Shipped,
    /// Some places pin exact builds and none asks for newer builds.
    Pinned,
    /// The operator set the scheduled interval to zero.
    Disabled,
    /// At least one place requests newer builds from a known channel.
    Newer,
    /// Newer builds were requested for kinds without a known release channel.
    NoChannel,
}

/// The last successful channel read and the current reason for its schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessChannelsStatus {
    /// When at least one release channel was last read successfully, if ever.
    pub last_read_at: Option<Time>,
    /// Why the schedule reads or does not read channels now.
    pub reason: HarnessChannelSchedule,
    /// The operator's interval in hours; zero keeps the schedule off.
    pub interval_hours: i64,
    /// The harness kinds whose effective place settings request newer builds.
    pub requested: Vec<String>,
}

/// A build of a harness that the tool cache can fetch. The Linux builds are for managed runtimes;
/// the others are for connected machines whose owner lets the connector fetch what the server
/// names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessBuild {
    /// The harness's kind.
    pub kind: String,
    /// The version.
    pub version: String,
    /// The operating system it is for.
    pub os: MachineOs,
    /// The processor it is for.
    pub arch: HarnessArch,
    /// Where it is fetched from.
    pub url: String,
    /// The SHA-256 of what is fetched, in lower case hex. For a package a registry gives only an integrity
    /// value for, it is the SHA-256 of what Orch fetched and verified against that value and the registry's
    /// signature when it read the channel.
    pub sha256: String,
    /// Whether it is the one used unless a profile names another.
    pub is_default: bool,
    /// Where it came from.
    pub source: HarnessBuildSource,
    /// How far it is known to work with its adapter; set by Orch, never by the request that adds a build.
    #[serde(default)]
    pub status: HarnessBuildStatus,
    /// Its last check, where one ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<HarnessBuildCheck>,
    /// What switches do to a history this build cannot take over natively, worked out for the build as its check
    /// left it; set when the build is served, never kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carry: Option<crate::programs::CarryFallback>,
}

/// A program actually held by a unit, with the check of that immutable build when known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunningHarnessBuild {
    /// The program kind.
    pub kind: String,
    /// The declared localized name, when this server knows the program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<crate::resources::HarnessKindName>,
    /// The version held by the unit.
    pub version: String,
    /// The matching catalogue build; null when its origin is not known to this server.
    pub build: Option<HarnessBuild>,
}

/// Units holding one program build, with the subset whose effective target differs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessBuildUsage {
    /// The program kind.
    pub kind: String,
    /// The held version.
    pub version: String,
    /// Visible environments holding it.
    pub environments: Vec<crate::environment::EnvironmentRef>,
    /// Visible provider runtimes holding it.
    pub providers: Vec<crate::resources::ProviderRef>,
    /// Environments behind their own effective policy or profile requirements.
    pub update_environments: Vec<Id>,
    /// Providers behind their own effective policy or profile requirements.
    pub update_providers: Vec<Id>,
}

/// The place whose version manager explicitly asks to fetch an exact vendor build.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HarnessVersionFetchBody {
    /// An environment managed by the caller; omitted for a node or system setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<Id>,
    /// An org or project whose settings the caller manages; omitted for environment or system.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<Id>,
}

/// How a plugin fares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    /// It runs.
    Running,
    /// It is being started again.
    Restarting,
    /// It failed.
    Failed,
}

/// An external implementation loaded on a component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminPlugin {
    /// The component that loaded it.
    pub component: String,
    /// The machine it runs on, where it is not the server.
    pub machine: Option<MachineRef>,
    /// What kind of extension it is.
    pub kind: String,
    /// Its name.
    pub name: String,
    /// Its version.
    pub version: String,
    /// How it fares.
    pub state: PluginState,
    /// More about its state, such as the last error.
    pub detail: String,
}

/// The people to list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdminUserListQuery {
    /// Only those whose name, username or address holds this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// Only those in this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AdminUserStatus>,
    /// Only system admins, or only those who are none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_admin: Option<bool>,
    /// How many entries, at most [`crate::common::MAX_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// The two limits of a root org: whether its tree takes further members, and whether it holds orgs
/// inside it. Setting one removes nobody and no org.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgLimitsChange {
    /// Whether its tree takes no further member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed: Option<bool>,
    /// Whether it holds no org inside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flat: Option<bool>,
}

/// Makes a person, with a personal org only when enabled. `personal_org` overrides
/// the enabled home's policies; it is invalid when personal-home creation is disabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdminUserCreateBody {
    /// The name they sign in with.
    pub username: String,
    /// The name shown beside what they do.
    pub display_name: String,
    /// Their address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Whether they are a system admin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_system_admin: Option<bool>,
    /// How their own org starts, where that differs from the system's policies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub personal_org: Option<OrgLimitsChange>,
}

/// The person that was made, and the link with which they set a password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminUserCreateReply {
    /// The person.
    pub user: AdminUser,
    /// The link to give them.
    pub reset_url: String,
    /// When the link stops being redeemable, according to the server's clock.
    pub expires_at: Time,
}

/// Changes a person. Making somebody a system admin, or any change to one, needs
/// `acknowledge_admin_risk`; the last system admin cannot be demoted or suspended.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdminUserUpdateBody {
    /// Whether they may sign in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AdminUserStatus>,
    /// Whether they are a system admin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_system_admin: Option<bool>,
    /// That the caller knows what it means to touch a system admin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledge_admin_risk: Option<bool>,
}

/// Makes a link with which a person sets a new password.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdminUserResetLinkCreateBody {
    /// Whether using the link also switches that person's second step off, an authenticator and a
    /// mailed code alike: for somebody who lost theirs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_second_step: Option<bool>,
}

/// A link to give a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminUserResetLinkCreateReply {
    /// The link.
    pub url: String,
    /// When it stops being good.
    pub expires_at: Time,
}

/// Sets a setting of the system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdminSettingSetBody {
    /// The value.
    pub value: Json,
    /// Whether to lock it for every node below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
    /// The own revision reviewed by the writer; zero requires an unset own value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

/// Removes only the system value the writer reviewed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdminSettingDeleteQuery {
    /// The own revision to remove, when a guarded reset or Undo is requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<Revision>,
}

/// Which runtimes to list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdminRuntimeListQuery {
    /// Only those on this host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<Id>,
    /// Only those of this kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<UnitKind>,
}

/// Which stored series to read. A span inside `[retention] metrics_fine_days` is answered from the
/// values of each minute; a longer or an older one from those of each hour, at a step of an hour or a
/// day (`400 request.invalid` for a finer step there). Without `name`, the answer lists the names
/// there are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdminMetricListQuery {
    /// A name of the metrics catalogue without its prefix; repeatable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<Vec<String>>,
    /// The start of the span; any span, since the history is kept for good.
    pub from: Time,
    /// The end of the span.
    pub to: Time,
    /// The width of a point, in minutes: 1, 5, 60 or 1 440. By the span when left out, so that a
    /// series has at most [`METRIC_MAX_POINTS`] points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
}

/// What the audit trail is asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdminAuditListQuery {
    /// Include agent acts for this person as well as their direct acts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_agents: Option<bool>,
    /// Only this exact meaning of act.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<crate::nodes::AuditKind>,
    /// Only what this person did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<Id>,
    /// Only this action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Only what concerns this node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<Id>,
    /// Only rows from this moment on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Time>,
    /// Only rows up to this moment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Time>,
    /// How many entries, at most [`crate::common::MAX_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// How the audit trail is written out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditExportFormat {
    /// Comma-separated values.
    Csv,
    /// One JSON object a line.
    Jsonl,
}

/// What is written out of the audit trail. The export is itself audited as `audit.exported`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdminAuditExportQuery {
    /// Only this person's acts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<Id>,
    /// Only this action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Only acts within this event-time place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<Id>,
    /// Include agent acts for this person as well as their direct acts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_agents: Option<bool>,
    /// Only this exact meaning of act.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<crate::nodes::AuditKind>,
    /// The format.
    pub format: AuditExportFormat,
    /// The start of the span.
    pub from: Time,
    /// The end of the span, at most [`AUDIT_EXPORT_MAX_DAYS`] days after `from`.
    pub to: Time,
}

/// The orgs to list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdminOrgListQuery {
    /// Only those whose name holds this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// With `only`: those deleted and not purged yet, which a system admin may restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<DeletedFilter>,
    /// Only team orgs, or only people's own; both when absent, the teams first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_kind: Option<OrgType>,
    /// How many entries, at most [`crate::common::MAX_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Changes an identity provider; what is left out stays.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdentityProviderUpdateBody {
    /// A new key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// A new label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A new issuer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    /// A new client identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// A new client secret; write-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// New scopes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    /// A new claim for the person's identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_subject: Option<String>,
    /// A new claim for their address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_email: Option<String>,
    /// A new claim for their name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_name: Option<String>,
    /// A new claim for their username.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_username: Option<String>,
    /// New domains of addresses let in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_email_domains: Option<Vec<String>>,
    /// Whether a user is made at the first sign-in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_register: Option<bool>,
    /// A new org joined at auto-registration; `null` for none.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<Id>")]
    pub default_org_id: Option<Option<Id>>,
    /// A new role in that org; `null` for none.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<Role>")]
    pub default_role: Option<Option<Role>>,
    /// Whether it is offered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// The revision the caller saw.
    pub expected_revision: Revision,
}

/// What a test of an identity provider found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IdentityProviderTestReply {
    /// Whether discovery worked.
    pub ok: bool,
    /// What went wrong, when it did not.
    pub error: Option<String>,
    /// The issuer's metadata, as fetched.
    pub issuer_metadata: Option<Json>,
}

/// Where an org's index of search by meaning stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SemanticState {
    /// Search by meaning is off.
    Off,
    /// It is on, and no embeddings provider and model are named.
    NotNamed,
    /// The database cannot keep vectors.
    Unavailable,
    /// Every piece has its vector.
    Idle,
    /// Pieces are being cut or made into vectors.
    Indexing,
    /// Indexing waits for what its reason names: the disk share, the provider, its machine, a cap or access.
    Paused,
    /// A new model waits for an admin's choice: the disk share cannot hold both generations.
    Waiting,
}

/// One generation of an org's vectors: what one model made, one way, of its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SemanticGeneration {
    /// Its identifier.
    pub id: Id,
    /// The provider whose endpoint makes its vectors.
    pub provider_id: Id,
    /// The model, by the name that is sent.
    pub model_name: String,
    /// The length asked of the model; `null` for its own.
    pub asked_dimensions: Option<i32>,
    /// The length of its vectors, once the first came back.
    pub dimensions: Option<i32>,
    /// What was put before a query.
    pub query_prefix: String,
    /// What was put before a document.
    pub document_prefix: String,
}

/// Where search by meaning stands for one root org.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SemanticSearchOrg {
    /// The root org.
    pub org: NodeRef,
    /// Whether search by meaning is on there.
    pub enabled: bool,
    /// The embeddings provider named, if any.
    pub provider_id: Option<Id>,
    /// The model named, if any.
    pub model_name: Option<String>,
    /// The length asked for, if any.
    pub dimensions: Option<u32>,
    /// Where its index stands.
    pub state: SemanticState,
    /// Why it pauses or waits, as a word: `disk_share`, `provider`, `model`, `access`, `cap`, `machine`,
    /// `extension`, `choice` or `error`; `more` while it indexes.
    pub reason: Option<String>,
    /// That reason in a sentence.
    pub detail: Option<String>,
    /// The generation that answers.
    pub active: Option<SemanticGeneration>,
    /// The generation being built beside it.
    pub building: Option<SemanticGeneration>,
    /// Estimated tokens of everything cut.
    pub tokens: i64,
    /// Of those, the tokens still without their vector in the generation being filled.
    pub pending_tokens: i64,
    /// The endpoint's measured tokens a second.
    pub tokens_per_second: Option<f64>,
    /// How long the pending tokens take at that rate, in seconds.
    pub seconds_left: Option<i64>,
    /// The bytes of the text it indexes.
    pub content_bytes: i64,
    /// The bytes the index takes.
    pub used_bytes: i64,
    /// The most the disk share lets it take; `null` for no limit.
    pub allowed_bytes: Option<i64>,
    /// While a new model waits for the choice: the bytes its vectors take beside the present ones.
    pub rebuild_bytes: Option<i64>,
    /// And the seconds it takes to make them at the measured rate.
    pub rebuild_seconds: Option<i64>,
}

/// Search by meaning across the system.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SemanticSearchOverview {
    /// Whether the database can keep vectors.
    pub available: bool,
    /// Why not: `absent` (no pgvector) or `not_permitted`.
    pub unavailable_reason: Option<String>,
    /// How often the indexer looks for work, in seconds: how often this is worth reading while it indexes.
    pub interval_secs: i64,
    /// The root orgs where it is on or that have an index, by name.
    pub orgs: Vec<SemanticSearchOrg>,
}

/// The active generation an admin chose to keep, with its model, prompts and chunking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticSearchKeepBody {
    /// The generation shown when the choice was made; a changed generation refuses the request.
    pub generation_id: Id,
}

crate::endpoints! {
    /// Lists the people of the system with what only a system admin reads about them.
    AdminUserList: GET "/admin/users", Session, query AdminUserListQuery, none, json Page<AdminUser>, 200;
    /// Makes a person, their optional personal org, and a link to set a password.
    AdminUserCreate: POST "/admin/users", Session, none, json AdminUserCreateBody, json AdminUserCreateReply, 200;
    /// Suspends a person or lets them sign in again, or changes whether they are a system admin.
    AdminUserUpdate: PATCH "/admin/users/{id}", Session, none, json AdminUserUpdateBody, json AdminUser, 200;
    /// Makes a link with which a person sets a new password, that may also switch their second step off.
    AdminUserResetLinkCreate: POST "/admin/users/{id}/reset-link", Session, none, json AdminUserResetLinkCreateBody, json AdminUserResetLinkCreateReply, 200;
    /// Gives a person one invitation to make a team org, which making one uses up while `orgs.created_by` is `invited`; a second one before the first is used changes nothing.
    AdminUserOrgInvitationGive: POST "/admin/users/{id}/org-invitation", Session, none, none, none, 204;
    /// Takes back the invitation to make a team org that a person has not used.
    AdminUserOrgInvitationTakeBack: DELETE "/admin/users/{id}/org-invitation", Session, none, none, none, 204;
    /// Lists the settings at system scope, resolved.
    AdminSettingList: GET "/admin/settings", Session, none, none, json Vec<ResolvedSetting>, 200;
    /// Counts a system setting's readable reach and lists the places that override it.
    AdminSettingReachGet: GET "/admin/settings/{key}/reach", Session, query PageQuery, none, json SettingReach, 200;
    /// Sets a setting at system scope, locked for every node below or not.
    AdminSettingSet: PUT "/admin/settings/{key}", Session, none, json AdminSettingSetBody, json ResolvedSetting, 200;
    /// Takes a setting at system scope back to what Orch ships.
    AdminSettingDelete: DELETE "/admin/settings/{key}", Session, query AdminSettingDeleteQuery, none, json ResolvedSetting, 200;
    /// Reads the defaults page of the system, which every org starts from.
    AdminDefaultsGet: GET "/admin/defaults", Session, none, none, json PermissionPage, 200;
    /// Changes the defaults page of the system; `whole: 'reset'` puts back what Orch ships.
    AdminDefaultsUpdate: PATCH "/admin/defaults", Session, none, json PageChange, json PermissionPage, 200;
    /// Says how many pages, in how many orgs, follow the rows of a change to the defaults.
    AdminDefaultsPreview: POST "/admin/defaults/preview", Session, none, json PageChange, json PagePreview, 200;
    /// Lists every host on the system; hosts themselves are managed through their own requests.
    AdminHostList: GET "/admin/hosts", Session, none, none, json Vec<Host>, 200;
    /// Lists the runtimes of both pools, whoever's they are.
    AdminRuntimeList: GET "/admin/runtimes", Session, query AdminRuntimeListQuery, none, json Vec<Runtime>, 200;
    /// Reads what the system stored about itself as series over a span.
    AdminMetricList: GET "/admin/metrics", Session, query AdminMetricListQuery, none, json Vec<MetricSeries>, 200;
    /// Makes an agent profile of the system, whose profile has no home.
    AdminAgentCreate: POST "/admin/agents", Session, none, json OrgAgentCreateBody, json Agent, 200;
    /// Lists the external implementations loaded anywhere, with how they fare.
    AdminPluginList: GET "/admin/plugins", Session, none, none, json Vec<AdminPlugin>, 200;
    /// Lists the definitions of capabilities.
    AdminCapabilityList: GET "/admin/capabilities", Session, none, none, json Vec<CapabilityDef>, 200;
    /// Writes the definition of a capability, adding it or replacing it.
    AdminCapabilitySet: PUT "/admin/capabilities/{key}", Session, none, json CapabilityDef, json CapabilityDef, 200;
    /// Removes the definition of a capability.
    AdminCapabilityDelete: DELETE "/admin/capabilities/{key}", Session, none, none, json CapabilityDef, 200;
    /// Lists the builds of harnesses that the tool cache can fetch.
    AdminHarnessList: GET "/admin/harnesses", Session, none, none, json Vec<HarnessBuild>, 200;
    /// Lists immutable build metadata for choosing versions and reviewing a target.
    HarnessBuildList: GET "/harnesses/builds", User, none, none, json Vec<HarnessBuild>, 200;
    /// Fetches one exact vendor version with its checksum and begins its background check.
    HarnessVersionFetch: POST "/harnesses/{kind}/{version}/fetch", Rule("manage the named environment or node settings, or system administration"), none, json HarnessVersionFetchBody, json Vec<HarnessBuild>, 202;
    /// Lists the visible units holding each build and those behind their effective version choice.
    AdminHarnessUsageList: GET "/admin/harnesses/usage", Session, none, none, json Vec<HarnessBuildUsage>, 200;
    /// Lists the program builds held by this environment even when no update is pending.
    EnvironmentHarnessBuildList: GET "/environments/{id}/harnesses", Permission("environment.view"), none, none, json Vec<RunningHarnessBuild>, 200;
    /// Adds or replaces a build of a harness and lists them all.
    AdminHarnessSet: PUT "/admin/harnesses/{kind}/{version}", Session, none, json HarnessBuild, json Vec<HarnessBuild>, 200;
    /// Reads the vendors' release channels now (as the schedule does) and lists the builds.
    AdminHarnessChannelsRead: POST "/admin/harnesses/channels", Session, none, none, json Vec<HarnessBuild>, 200;
    /// Reads when release channels were last read and why they are or are not read on a schedule.
    AdminHarnessChannelsGet: GET "/admin/harnesses/channels", Session, none, none, json HarnessChannelsStatus, 200;
    /// Imports a carried-in vendor checksum list and audits its digest.
    AdminHarnessChecksumsImport: POST "/admin/harnesses/checksums", Session, none, json HarnessChecksumsImportBody, json HarnessChecksumsImportReply, 200;
    /// Checks every kept build of a harness at a version again, in the background, and lists the builds.
    AdminHarnessCheck: POST "/admin/harnesses/{kind}/{version}/check", Session, none, none, json Vec<HarnessBuild>, 200;
    /// Reads the audit trail of the whole system.
    AdminAuditList: GET "/admin/audit", Session, query AdminAuditListQuery, none, json Page<AuditEntry>, 200;
    /// Writes a span of the audit trail, at most 366 days, as a streamed file.
    AdminAuditExport: GET "/admin/audit/export", Session, query AdminAuditExportQuery, none, raw, 200;
    /// Lists every org on the system; with `deleted=only`, those deleted and not purged yet.
    AdminOrgList: GET "/admin/orgs", Session, query AdminOrgListQuery, none, json Page<Org>, 200;
    /// Sets the two limits of a root org, a personal one or any other: whether its tree takes further members, and whether it holds orgs inside it.
    AdminOrgUpdate: PATCH "/admin/orgs/{id}", Session, none, json OrgLimitsChange, json Org, 200;
    /// Lists the identity providers.
    AdminIdentityProviderList: GET "/admin/identity-providers", Session, none, none, json Vec<IdentityProvider>, 200;
    /// Adds an identity provider.
    AdminIdentityProviderCreate: POST "/admin/identity-providers", Session, none, json IdentityProviderInput, json IdentityProvider, 200;
    /// Changes an identity provider.
    AdminIdentityProviderUpdate: PATCH "/admin/identity-providers/{id}", Session, none, json IdentityProviderUpdateBody, json IdentityProvider, 200;
    /// Deletes an identity provider, which is refused while it is the only sign-in method of any active user.
    AdminIdentityProviderDelete: DELETE "/admin/identity-providers/{id}", Session, none, none, none, 204;
    /// Fetches the discovery document of an identity provider's issuer, to see that it works.
    AdminIdentityProviderTest: POST "/admin/identity-providers/{id}/test", Session, none, none, json IdentityProviderTestReply, 200;
    /// Reads what the blob store holds.
    AdminStorageGet: GET "/admin/storage", Session, none, none, json StorageStats, 200;
    /// Reads the state of the system's certificates.
    AdminPkiGet: GET "/admin/pki", Session, none, none, json PkiInfo, 200;
    /// Reads the state of the system: its release, its parts and its queues.
    AdminSystemGet: GET "/admin/system", Session, none, none, json SystemInfo, 200;
    /// Checks only the operator's configured source and installs nothing.
    AdminReleaseCheck: POST "/admin/releases/check", Session, none, none, json ReleaseCheck, 200;
    /// Reconciles protected host metadata before returning operational facts.
    AdminInstallationRecordsGet: GET "/admin/installation-records", Session, none, none, json InstallationRecords, 200;
    /// Reads the exact verified backup manifest's restore inventory; none without an available verified manifest.
    AdminBackupRestoreList: GET "/admin/backup-restore-list", Session, none, none, json Option<BackupRestoreList>, 200;
    /// Reads where search by meaning stands for each root org where it is on or that has an index.
    AdminSemanticSearchGet: GET "/admin/semantic-search", Session, none, none, json SemanticSearchOverview, 200;
    /// Rebuilds a root org's index in place: its vectors are removed and made anew with the model named.
    AdminSemanticSearchRebuild: POST "/admin/semantic-search/{id}/rebuild", Session, none, none, json SemanticSearchOrg, 200;
    /// Keeps the present generation's model, prompts and chunk size, respecting the org's settings and locks.
    AdminSemanticSearchKeep: POST "/admin/semantic-search/{id}/keep", Session, none, json SemanticSearchKeepBody, json SemanticSearchOrg, 200;
    /// Reads the OpenAPI 3.1 document, generated from the same types as this contract.
    OpenapiGet: GET "/openapi.json", User, none, none, raw, 200;
    /// Reads the certificate of the system's authority.
    PkiCaGet: GET "/pki/ca.pem", Public, none, none, raw, 200;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_index_of_meaning_reads_back_as_written_with_its_progress_and_disk() {
        let written = json!({
            "available": true, "unavailable_reason": null, "interval_secs": 10,
            "orgs": [{
                "org": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01", "kind": "org", "name": "Acme", "redacted": false },
                "enabled": true, "provider_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a02",
                "model_name": "embeddinggemma-2", "dimensions": 256, "state": "waiting",
                "reason": "choice", "detail": "past the disk share",
                "active": {
                    "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a03", "provider_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a02",
                    "model_name": "embeddinggemma-2", "asked_dimensions": 256, "dimensions": 256,
                    "query_prefix": "task: search result | query: ", "document_prefix": "title: {title} | text: "
                },
                "building": null, "tokens": 120000, "pending_tokens": 0, "tokens_per_second": 212.5,
                "seconds_left": 0, "content_bytes": 480000, "used_bytes": 260000, "allowed_bytes": 480000,
                "rebuild_bytes": 300000, "rebuild_seconds": 565
            }]
        });
        let read: SemanticSearchOverview = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(read.orgs[0].state, SemanticState::Waiting);
        assert_eq!(read.orgs[0].rebuild_seconds, Some(565));
        assert_eq!(serde_json::to_value(read).unwrap(), written);
    }

    #[test]
    fn certificates_ending_soon_name_the_machine_and_date_beside_the_full_count() {
        let written = json!({
            "ca": {
                "subject": "Orch authority",
                "not_before": "2026-01-01T00:00:00.000Z",
                "not_after": "2036-01-01T00:00:00.000Z",
                "fingerprint_sha256": "a".repeat(64)
            },
            "services": [],
            "machines_expiring_30d": 51,
            "machines_expiring": [{
                "machine": {
                    "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01",
                    "name": "Build host", "online": false,
                    "platform_os": "linux", "placement": "managed"
                },
                "not_after": "2026-10-20T00:00:00.000Z"
            }]
        });
        let pki: PkiInfo = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(pki.machines_expiring_30d, 51);
        assert_eq!(pki.machines_expiring[0].machine.name, "Build host");
        assert_eq!(serde_json::to_value(pki).unwrap(), written);
    }

    #[test]
    fn a_person_is_read_with_the_proofs_that_strangers_locked() {
        let written = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01", "kind": "human", "username": "bo",
            "display_name": "Bo", "avatar": { "text": "B", "color": 2 },
            "email": "bo@example.com", "email_verified": true, "status": "active",
            "is_system_admin": false, "totp_enabled": true, "email_code_second_step": false,
            "locked": [{ "way": "email_code", "until": "2026-10-08T20:00:00.000Z" }],
            "org_invited_at": null,
            "created_at": "2026-09-01T08:00:00.000Z", "last_seen_at": null
        });
        let user: AdminUser = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(user.locked[0].way, SignInProof::EmailCode);
        assert_eq!(user.base.username, "bo");
        assert_eq!(serde_json::to_value(&user).unwrap(), written);
    }

    #[test]
    fn an_identity_provider_never_returns_its_secret_and_is_changed_by_revision() {
        let made: IdentityProviderInput = serde_json::from_value(json!({
            "key": "corp", "name": "Corporate sign-in", "issuer": "https://id.example.com",
            "client_id": "orch", "client_secret": "s3cret", "default_role": "viewer"
        }))
        .unwrap();
        assert_eq!(made.default_role, Some(Role::Viewer));
        let shown = IdentityProvider {
            id: orch_types::new_id(),
            key: made.key.clone(),
            name: made.name.clone(),
            issuer: made.issuer.clone(),
            client_id: made.client_id.clone(),
            scopes: vec!["openid".into()],
            claim_subject: "sub".into(),
            claim_email: "email".into(),
            claim_name: "name".into(),
            claim_username: "preferred_username".into(),
            allowed_email_domains: vec![],
            auto_register: false,
            default_org_id: None,
            default_role: Some(Role::Viewer),
            enabled: true,
            client_secret_set: true,
            callback_url: "https://orch.example.com/api/v1/auth/oidc/corp/callback".into(),
            revision: 1,
        };
        let written = serde_json::to_value(&shown).unwrap();
        assert!(written.get("client_secret").is_none());
        assert_eq!(written["client_secret_set"], json!(true));
        let clear: IdentityProviderUpdateBody =
            serde_json::from_value(json!({ "default_org_id": null, "expected_revision": 1 }))
                .unwrap();
        assert_eq!(clear.default_org_id, Some(None));
        assert_eq!(clear.default_role, None);
        assert!(
            serde_json::from_value::<IdentityProviderUpdateBody>(json!({ "enabled": false }))
                .is_err()
        );
    }

    #[test]
    fn a_harness_build_is_for_an_os_and_a_processor() {
        let build: HarnessBuild = serde_json::from_value(json!({
            "kind": "codex", "version": "1.2.3", "os": "linux", "arch": "aarch64",
            "url": "https://example.com/codex", "sha256": "a".repeat(64), "is_default": true, "source": "admin"
        }))
        .unwrap();
        assert_eq!(build.os, MachineOs::Linux);
        assert_eq!(build.arch, HarnessArch::Aarch64);
        assert_eq!(
            serde_json::to_value(HarnessArch::X86_64).unwrap(),
            json!("x86_64")
        );
    }

    #[test]
    fn a_series_keeps_its_points_as_pairs_of_a_moment_and_a_value() {
        let written = json!({
            "name": "outbox_pending", "labels": { "channel": "email" }, "resolution": "hour",
            "points": [["2026-10-08T17:00:00.000Z", 3.0], ["2026-10-08T18:00:00.000Z", 4.5]],
            "peaks": [["2026-10-08T17:00:00.000Z", 9.0], ["2026-10-08T18:00:00.000Z", 7.0]]
        });
        let series: MetricSeries = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(series.points.len(), 2);
        assert_eq!(series.resolution, MetricResolution::Hour);
        assert_eq!(serde_json::to_value(&series).unwrap(), written);
    }

    #[test]
    fn the_limits_of_an_org_are_changed_one_at_a_time() {
        let closed: OrgLimitsChange = serde_json::from_value(json!({ "closed": true })).unwrap();
        assert_eq!((closed.closed, closed.flat), (Some(true), None));
        assert!(serde_json::from_value::<OrgLimitsChange>(json!({ "open": true })).is_err());
        let made: AdminUserCreateBody = serde_json::from_value(json!({
            "username": "ann", "display_name": "Ann", "personal_org": { "flat": true }
        }))
        .unwrap();
        assert_eq!(made.personal_org.and_then(|limits| limits.flat), Some(true));
    }
}

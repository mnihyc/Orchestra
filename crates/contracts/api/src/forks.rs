//! Fork, switch and move plans: history up to any turn, files, the agent's notes, fidelity, estimates, options, who owns the result and who of the people on the page may go on, what is locked and what is left behind; the update plan of a provider's runtime.
//!
//! A fork leaves a thread as it is and makes a new one; a switch is the same
//! operation whose result takes the source's place. Both are planned first: a
//! [`ForkRequest`] gets a [`ForkPlan`] that says how the history travels
//! ([`HistoryReport`]), which files are copied, whose the result is, who of
//! the people on the page can go on using it, what stands still meanwhile and
//! what is left behind, with an [`Estimate`] and the blockers that stop it.
//! The plan is good for fifteen minutes and is carried out by a [`Job`].
//! Moving an environment to another host and updating the unit of an
//! environment or of a provider's runtime are planned the same way.

use std::collections::BTreeMap;

use orch_types::{Id, Json, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::{AccessSummary, Dependency, Holder, MissingAccess, ThreadAccess};
use crate::binding::{ArrangementType, HostRef, Isolation};
use crate::nodes::{EnvironmentRef, NodeRef};
use crate::routes::Access::Rule;
use crate::threads::{AgentRef, ProviderRef};
use crate::users::UserRef;

/// How long a plan stays good, in minutes; a later request to carry it out is
/// refused as out of date.
pub const PREFLIGHT_VALID_MINUTES: i64 = 15;
/// How well a history that was taken over was kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Fidelity {
    /// All of it, as it was.
    Lossless,
    /// Its structure, converted.
    Structured,
    /// A summary written for the new agent.
    Summary,
}

/// What became of the items of one kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportKind {
    /// Kept as they were.
    pub kept: i64,
    /// Converted.
    pub converted: i64,
    /// Dropped.
    pub dropped: i64,
}

/// Items of one kind that were dropped, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportDropped {
    /// The kind.
    pub kind: String,
    /// How many.
    pub count: i64,
    /// Why.
    pub reason: String,
}

/// What was kept, converted and dropped when a history travelled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryReport {
    /// How well it was kept.
    pub fidelity: Fidelity,
    /// How many items there were.
    pub items_total: i64,
    /// What became of the items of each kind.
    pub by_kind: BTreeMap<String, ReportKind>,
    /// What was dropped, and why.
    pub dropped: Vec<ReportDropped>,
    /// Anything the reader should know.
    pub warnings: Vec<String>,
}

/// How an estimate was arrived at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EstimateBasis {
    /// From what this system measured before.
    Measured,
    /// From a guess.
    Assumed,
}

/// How long something takes, as a range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Estimate {
    /// At the least.
    pub seconds_low: i64,
    /// At the most.
    pub seconds_high: i64,
    /// What it rests on.
    pub basis: EstimateBasis,
}

/// How paths change when a workspace moves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PathMap {
    /// The root before.
    pub from: String,
    /// The root after.
    pub to: String,
}

/// What a fork is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ForkResult {
    /// A thread of its own.
    New,
    /// This thread goes on as the result: a switch.
    Replace,
}

/// How much of the history goes along.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HistoryMode {
    /// All of it.
    All,
    /// A range of turns.
    Range,
    /// A summary written for the new agent.
    Summary,
    /// Nothing: a fresh thread that only shares the binding or the files.
    #[serde(rename = "none")]
    Nothing,
}

/// The history a fork or a switch takes along.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForkHistory {
    /// How much.
    pub mode: HistoryMode,
    /// For a range: the first turn, by number; the first turn when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_turn: Option<i64>,
    /// For a range: the last turn; the last completed turn when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_turn: Option<i64>,
}

/// What the result runs on; each member left out stays as it is on the source.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForkTo {
    /// The provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<Id>,
    /// The agent profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<Id>,
    /// The model, by its routing name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// How the agent is placed. What carries out its tool calls is not asked for: the environment
    /// it lands in decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrangement: Option<ArrangementType>,
}

/// What to do about a turn that is running when the change is confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActiveTurn {
    /// Wait for it.
    Wait,
    /// Interrupt it.
    Interrupt,
}

/// The parameters of a change; each one left out takes its setting.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangeOptions {
    /// How to treat a turn that is under way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn: Option<ActiveTurn>,
    /// With waiting: how long to wait before giving up with nothing changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_minutes: Option<i64>,
    /// How long the health checks may take.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_timeout_secs: Option<i64>,
    /// Keep what is left behind rather than scheduling it for removal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_previous: Option<bool>,
    /// For a switch out of an environment that others stay in: end this
    /// thread's processes there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_processes: Option<bool>,
}

/// The values of a change that will apply: the request's, else the settings'.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AppliedChangeOptions {
    /// How to treat a turn that is under way.
    pub active_turn: ActiveTurn,
    /// How long to wait for it.
    pub wait_minutes: i64,
    /// How long the health checks may take.
    pub verify_timeout_secs: i64,
    /// Whether what is left behind is kept.
    pub keep_previous: bool,
    /// Whether this thread's processes are ended.
    pub stop_processes: bool,
}

/// Which files go along.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileSelection {
    /// All of them.
    All,
    /// None.
    #[serde(rename = "none")]
    Nothing,
    /// Those that match.
    Selection {
        /// Patterns of what to take.
        include: Vec<String>,
        /// Patterns of what to leave out of that.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exclude: Option<Vec<String>>,
    },
}

/// A new environment for a fork or a switch: made now, owned by the caller.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForkNewEnvironment {
    /// Its name, unique among the caller's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The place whose settings it takes: a topic, a project or an org.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_node_id: Option<Id>,
    /// The template; what its home resolves when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<Id>,
    /// The host: what its home resolves when left out, and wherever there is
    /// most room when `null`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::common::absent_or_null"
    )]
    #[schemars(with = "Option<Id>")]
    pub host_id: Option<Option<Id>>,
    /// Values of its own for the keys whose scope is environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Json>,
    /// Who may use it; by its home's defaults when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<crate::binding::NewEnvironmentAccess>,
    /// Which of the source's files it starts with; all when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<FileSelection>,
    /// Whether the home directory goes along; no when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<bool>,
    /// Whether what was written in private views goes along; yes when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_views: Option<bool>,
}

/// Where the result of a fork or a switch goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum ForkEnvironment {
    /// Stay in the source's environment and directory; files are shared and not copied.
    Same {
        /// Always true.
        same: bool,
    },
    /// A different environment that the caller manages, used as it stands.
    Existing {
        /// The environment.
        #[serde(rename = "use")]
        environment_id: Id,
        /// Which files go along; all when left out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        files: Option<FileSelection>,
    },
    /// An environment made now, which is its maker's.
    New {
        /// What to make.
        new: ForkNewEnvironment,
    },
}

/// What a person asks for when they want a fork or a switch: one shape serves both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForkRequest {
    /// A thread of its own, or this thread as the result.
    pub result: ForkResult,
    /// For a new thread: the topic it is made in; the source's when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<Id>,
    /// For a new thread: its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// For a new thread: the page it starts with; the source's page, carried
    /// over, when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<ThreadAccess>,
    /// How much of the history; all of it when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<ForkHistory>,
    /// What it runs on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<ForkTo>,
    /// Where it goes; the source's environment when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<ForkEnvironment>,
    /// An own worktree on a new branch, for a new thread in the same environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<crate::source::WorktreeChoice>,
    /// Complete saved files at the chosen conversation point, copied into an unused directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_at: Option<ForkFilesAt>,
    /// Used when the directory differs: whether the agent's own notes are carried over. They are by
    /// default in a switch, and in a fork that takes history; a fork without history leaves them
    /// behind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<bool>,
    /// The thread's directory on the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// Where the agent starts in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// What to do about a turn that is running, and the rest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<ChangeOptions>,
    /// What becomes of each mount in a new environment; each one left out takes its manager's default. An
    /// environment that is used as it stands keeps its mounts as they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mounts: Option<Vec<MountChoice>>,
    /// Compare attached read-only synced locations with their source now; omitted or false reads no contents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_fingerprints: Option<bool>,
    /// Identifies one explicit content check across its live progress and subsequent replans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint_request_id: Option<Id>,
}

/// Where a copy goes when the target machine's location for the mount holds data already.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CopyInto {
    /// Into that location; what it holds is kept aside until somebody purges it.
    Replace,
    /// Into a new location, which becomes the machine's; the previous one is kept aside until somebody purges it.
    NewLocation,
}

/// What a request asks of one mount when its environment goes to another machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MountChoice {
    /// The mount.
    pub mount_id: Id,
    /// What becomes of it.
    pub fate: MountFate,
    /// For a copy into a location that holds data: replace that data, or copy into a new location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub into: Option<CopyInto>,
}

/// The recorded state explicitly chosen for the fork's files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForkFilesAt {
    /// The complete saved state in its owning environment.
    pub snapshot_id: Id,
}

/// An own-worktree base resolved before a fork starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForkPlanWorktree {
    /// The new directory, branch and source choice.
    pub choice: crate::source::WorktreeChoice,
    /// The resolved base commit.
    pub base_commit: String,
    /// Whether included saved file changes are carried.
    pub changes_carried: bool,
}

/// Carries out a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForkCreateBody {
    /// Explicit notification choice for operations that provide an affected-audience plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<bool>,
    /// The plan, as it was answered.
    pub preflight_id: Id,
    /// Options that differ from the plan's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<ChangeOptions>,
}

/// Optional notices when a reviewed runtime update interrupts other people's work.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateConfirmQuery {
    /// Send one notice per affected person for this operation; true when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<bool>,
}

/// The current audience of an update, without naming people in hidden threads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateAudience {
    /// Distinct visible people, excluding the person making the plan.
    pub people: Vec<crate::users::UserRef>,
    /// Additional affected people whose thread the reader may not view.
    pub hidden: i64,
}

/// Plans a switch of several threads at once. Exactly one of `thread_ids` and
/// `all_of_environment`, `all_of_provider` or `all_of_agent` is given.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForkBatchPreflightBody {
    /// The threads, within the resolved system batch setting, each switched in place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ids: Option<Vec<Id>>,
    /// Every thread of an environment that the caller manages, including those they cannot see. It
    /// goes with `environment` alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_of_environment: Option<Id>,
    /// Threads bound to this provider that the caller may switch. Others are counted as skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_of_provider: Option<Id>,
    /// Threads bound to this profile that the caller may switch. Others are counted as skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_of_agent: Option<Id>,
    /// Narrows all_of_provider to one model membership at that source provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_model_id: Option<Id>,
    /// What they run on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<ForkTo>,
    /// Where they go; a new environment is made once for all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<ForkEnvironment>,
    /// What to do about turns that are running, and the rest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<ChangeOptions>,
}

pub use crate::environment::{
    Amounts, EnvironmentReason, HostReason, HostStorage, HostWarning, PendingUpdate, Place,
    PlaceEnvironment, PlaceEnvironments, PlaceFacts, PlaceHost, PlaceMount, PlaceUsed, UpdateWhat,
    WhenFull,
};
pub use crate::mounts::{MountDataKind, MountFate, MountRef};

/// How the history of the source travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HistoryMethod {
    /// The same session goes on.
    SameSession,
    /// The harness forks its own session.
    NativeFork,
    /// The harness's session is carried over to the other.
    NativeTransplant,
    /// The history is converted into a neutral form and imported.
    NeutralImport,
    /// The full conversation is a file the new agent reads, without a model summary.
    FileHandoff,
    /// A summary is handed to the new agent.
    SummaryHandoff,
    /// Nothing travels.
    #[serde(rename = "none")]
    Nothing,
}

/// Where the files of a plan are read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FilesSource {
    /// From the environment itself.
    Live,
    /// From what was cached when it was last reachable.
    Cached,
}

/// Files that cannot be copied: the machine is offline and they are not cached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FilesMissing {
    /// How many.
    pub count: i64,
    /// How many bytes, where known.
    pub bytes: Option<i64>,
    /// A few of their paths.
    pub sample: Vec<String>,
}

/// The home directory that goes along.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Measurement {
    /// The background job counting this data; its ordinary live feed carries the counts.
    pub job_id: Id,
    /// When the complete count was taken; null while counting.
    pub measured_at: Option<Time>,
}

/// The home directory that goes along.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FilesHome {
    /// The reusable background measurement, when requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measurement: Option<Measurement>,
    /// Whether the old host measured this whole volume; an unavailable measurement blocks confirmation.
    #[serde(default)]
    pub measured: bool,
    /// How many files.
    pub file_count: i64,
    /// How many bytes.
    pub total_bytes: i64,
}

/// What was written in a private view of a mount, and whether it goes along.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrivateViewPlan {
    /// The reusable background measurement, when requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measurement: Option<Measurement>,
    /// The mount.
    pub mount: MountRef,
    /// Whether the old host measured the whole owned layer; false counts must not be shown as an empty layer.
    #[serde(default)]
    pub measured: bool,
    /// How many files.
    pub file_count: i64,
    /// How many bytes.
    pub total_bytes: i64,
    /// Whether it is carried.
    pub carried: bool,
    /// Why the target does not carry it; null when it is carried.
    pub left_because: Option<PrivateViewLeftBecause>,
}

/// Why an environment's private mount changes do not travel to the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PrivateViewLeftBecause {
    /// The person chose to leave the changes behind.
    Asked,
    /// The target has no location for the mount.
    NoLocation,
    /// The target attaches the mount in another mode.
    OtherMode,
    /// The target cannot make a private view.
    NoPrivateViews,
}

/// The files a plan copies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FilesPlan {
    /// Owned installed-file layers carried by an environment move; the full archive also includes its base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept_image_bytes: Option<i64>,
    /// Where they are read from.
    pub source: FilesSource,
    /// As of when, for cached files.
    pub source_as_of: Time,
    /// How many files.
    pub file_count: i64,
    /// How many bytes.
    pub total_bytes: i64,
    /// How many bytes go up to the server.
    pub bytes_to_upload: i64,
    /// How many bytes go down to the target.
    pub bytes_to_download: i64,
    /// What cannot be copied.
    pub missing: FilesMissing,
    /// How paths change.
    pub path_map: PathMap,
    /// The home directory, if it goes along.
    pub home: Option<FilesHome>,
    /// The mandatory owned file-state store on a whole-environment move; null for an ordinary file copy.
    #[serde(default)]
    pub file_states: Option<FilesPlanFileStates>,
    /// The private views of mounts.
    pub private_views: Vec<PrivateViewPlan>,
}

/// Persistent recorded-state data measured before a whole-environment move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FilesPlanFileStates {
    /// The reusable background measurement, when requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measurement: Option<Measurement>,
    /// Saved root metadata records.
    pub roots: i64,
    /// Owned immutable Git objects.
    pub objects: i64,
    /// Bytes carried independently of the choice for other home contents.
    pub total_bytes: i64,
    /// Restore, worktree, publication and materialization journals.
    pub journals: i64,
    /// Whether all persistent parts were measured.
    pub complete: bool,
}

impl From<orch_driver_contract::volumes::FileStatesMeasure> for FilesPlanFileStates {
    fn from(value: orch_driver_contract::volumes::FileStatesMeasure) -> Self {
        Self {
            measurement: None,
            roots: value.roots,
            objects: value.objects,
            total_bytes: value.total_bytes,
            journals: value.journals,
            complete: value.complete,
        }
    }
}

/// What a plan does with a mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MountAction {
    /// The mount stays attached.
    Keep,
    /// The mount is detached.
    Detach,
    /// The mount is newly attached.
    Attach,
}

/// What a plan does with one mount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanMount {
    /// The mount.
    pub mount: MountRef,
    /// What happens to it.
    pub action: MountAction,
    /// Where it will be, when it is kept or attached.
    pub path: Option<String>,
    /// The mode on the target; null when detached.
    pub mode: Option<crate::mounts::MountMode>,
    /// Why it is detached; null when kept or attached.
    pub detached_because: Option<MountDetachedBecause>,
    /// What becomes of it: what the request chose, else its manager's default where that can be done here.
    pub fate: MountFate,
    /// What its manager has plans preselect.
    pub default_fate: MountFate,
    /// What may be chosen for it in this plan, in the order they are offered. Only `as_is` where the result stays
    /// in an environment that is used as it stands.
    pub fates: Vec<MountFate>,
    /// Why its data cannot be copied to the target; null when it can.
    pub copy_unavailable: Option<CopyUnavailable>,
    /// The copy, when its fate is `copy`.
    pub copy: Option<PlanMountCopy>,
    /// What the data of its location on the source machine is, as its managers say; null when nobody said.
    pub data_kind: Option<MountDataKind>,
    /// Whether both locations can be compared and the target is an attached read-only synced copy.
    #[serde(default)]
    pub fingerprints_available: bool,
    /// The requested point-in-time comparison; null until checked, or when not applicable.
    pub fingerprints: Option<PlanMountFingerprints>,
    /// The background read requested by Check, retained for its live progress and cancellation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint_measurement: Option<Measurement>,
}

/// A comparison of a mount's source and target content at the stated time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanMountFingerprints {
    /// When both locations finished being read, from the server's clock.
    pub checked_at: Time,
    /// Whether their content fingerprints are equal.
    pub matches: bool,
}

/// Why a mount's data cannot be copied to a plan's target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CopyUnavailable {
    /// The result stays in an environment that is used as it stands; its mounts are not changed.
    Unchanged,
    /// The target is the machine the data is on.
    SameMachine,
    /// The source environment does not have it attached, so there is nothing to read.
    NotAttached,
    /// The source or the target cannot read or write a whole location: it is not a host, or its driver cannot
    /// copy whole volumes there.
    Unsupported,
}

/// What a copy carries to the target machine, and where it lands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanMountCopy {
    /// The reusable source measurement, when requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measurement: Option<Measurement>,
    /// Whether the source location was measured whole; an unmeasured copy cannot be confirmed.
    pub measured: bool,
    /// How many files it copies.
    pub file_count: i64,
    /// How many bytes.
    pub total_bytes: i64,
    /// The location it lands in.
    pub location: PlanCopyLocation,
    /// What the target machine's location holds now, when it holds anything; `into` is then required.
    pub holds: Option<PlanCopyHolds>,
    /// What the request chose for the data that is there.
    pub into: Option<CopyInto>,
    /// Whether the copy may replace what is there: not where that is a directory Orch did not make, nor while an
    /// environment on that machine has it attached.
    pub replace: bool,
}

/// The location a copy lands in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanCopyLocation {
    /// True when the copy makes it: a volume with a name of that machine's own; false for the machine's location.
    pub made: bool,
    /// A directory or a volume.
    pub kind: crate::mounts::MountLocationKind,
    /// The volume's name, or the directory; null unless the caller may change the machine's locations.
    pub source: Option<String>,
    /// For a volume, the host's storage it is on; null for its default.
    pub storage: Option<String>,
}

/// What the target machine's location of a mount holds before a copy lands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanCopyHolds {
    /// The reusable target measurement, when requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measurement: Option<Measurement>,
    /// Whether it was measured whole; when it was not, the counts are what is known.
    pub measured: bool,
    /// How many files.
    pub file_count: i64,
    /// How many bytes.
    pub total_bytes: i64,
}

/// Why a plan leaves a mount detached on its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MountDetachedBecause {
    /// The request detaches it.
    Asked,
    /// The target machine has no location for the mount.
    NoLocation,
    /// The target driver does not attach it.
    Driver,
    /// The target machine's policy refuses it.
    Refused,
}

/// What becomes of an MCP server when its environment goes elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpFate {
    /// It is there on both sides.
    Kept,
    /// It is not there on the target: imported from the source machine's harness configuration, the source
    /// environment's own, or not switched on there.
    Lost,
    /// It is switched on there and cannot start; `reason` says why.
    Unavailable,
    /// It is there only on the target.
    New,
}

/// One MCP server of a plan's source or target environment, and what it becomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanMcpServer {
    /// The server's identifier.
    pub id: Id,
    /// Its name, as the harness sees it.
    pub name: String,
    /// Whether it is the source environment's own.
    pub own: bool,
    /// Whether it was imported from a machine's harness configuration.
    pub imported: bool,
    /// What it becomes.
    pub fate: McpFate,
    /// Why it is lost or unavailable, in words; null when it is kept or new.
    pub reason: Option<String>,
}

/// Whether the agent's notes travel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForkPlanMemory {
    /// Whether they go along.
    pub carried: bool,
    /// How many files.
    pub files: i64,
    /// How many bytes.
    pub bytes: i64,
}

/// What the source runs on now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForkPlanFrom {
    /// The agent profile.
    pub agent: AgentRef,
    /// The provider.
    pub provider: ProviderRef,
    /// The environment, if it has one.
    pub environment: Option<EnvironmentRef>,
}

/// An environment to be made, and where.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlannedEnvironment {
    /// Its name.
    pub name: String,
    /// Where it would be made.
    pub place: Place,
}

/// The environment the result goes to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[expect(
    clippy::large_enum_variant,
    reason = "a plan is built once to be written; boxing the environment to be made would put a layer in every use"
)]
pub enum ForkPlanTarget {
    /// One that exists.
    Existing(EnvironmentRef),
    /// One to be made, which is the caller's.
    Made {
        /// What and where.
        new: PlannedEnvironment,
    },
}

/// What the result will run on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ForkPlanTo {
    /// The agent profile.
    pub agent: AgentRef,
    /// The provider.
    pub provider: ProviderRef,
    /// The model, by its routing name.
    pub model: String,
    /// The effort.
    pub effort: Option<String>,
    /// The approval mode the confirmed target will use, by its settings identifier.
    /// Older saved plans may omit this choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<String>,
    /// How the agent is placed.
    pub arrangement: ArrangementType,
    /// What that amounts to there.
    pub isolation: Isolation,
    /// Whether the arrangement and what it amounts to are predicted from the template of an environment that is
    /// not made yet: what it actually offers is checked before anything is committed, and a plan it cannot hold
    /// stops there with nothing left behind.
    pub predicted: bool,
    /// The environment.
    pub environment: ForkPlanTarget,
    /// The directory there.
    pub directory: String,
}

/// What a summary of the conversation will send to the summary model, and what that comes to, before it is confirmed.
/// Every figure is an estimate: the conversation is cut to the share of the model's window and is not written yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistorySummaryEstimate {
    /// Whether a summary can be made here: a summary model is chosen for the place.
    pub available: bool,
    /// The summary model's window in tokens; `null` where its entry does not say.
    pub window: Option<i64>,
    /// The share of the window that is sent at most, in percent (`summary.handoff_input_share`).
    pub share: i64,
    /// About how many tokens are sent: the newest turns in brief and the older ones as one line each, never more than
    /// the share.
    pub input_tokens: i64,
    /// About how many tokens come back, at most.
    pub output_tokens: i64,
    /// About what the call costs, in the place's credits, as a decimal string; `null` where no rate names the model.
    pub credits: Option<String>,
}

/// How the history travels in a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForkPlanHistory {
    /// The target build whose check disabled native import; Carry uses the remaining method.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_import_unavailable: Option<crate::admin::RunningHarnessBuild>,
    /// How much of it.
    pub mode: HistoryMode,
    /// How many turns.
    pub turns: i64,
    /// How it travels.
    pub method: HistoryMethod,
    /// Whether the source harness can export its native session now.
    pub source_available: bool,
    /// What is kept, converted and dropped.
    pub report: HistoryReport,
    /// How big the archive of it is, where there is one.
    pub archive_bytes: Option<i64>,
    /// For a summary: what it will send to the summary model and cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<HistorySummaryEstimate>,
}

/// A permission whose holders differ on the result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForkPlanDifference {
    /// The permission.
    pub permission: String,
    /// Who held it.
    pub before: Vec<Holder>,
    /// Who would.
    pub after: Vec<Holder>,
}

/// The page the result will have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForkPlanAccess {
    /// The page in three lines.
    pub summary: AccessSummary,
    /// Which of the defaults the result's agent, files and terminal rows follow there.
    pub profile: crate::access::PageProfile,
    /// For a fork: only what the rows that follow the defaults would give in another topic, and
    /// whatever the request itself changes. For a switch: what the inherited part gains or loses
    /// when the thread goes from a managed environment to a person's machine or back. Empty when
    /// nothing differs.
    pub differs: Vec<ForkPlanDifference>,
}

/// Whose the result is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForkPlanOwner {
    /// For a switch, the first owner before; `null` for a fork.
    pub before: Option<UserRef>,
    /// The first owner after: for a switch the same, for a fork the caller alone.
    pub after: UserRef,
}

/// What happens to the environment left behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LeftAction {
    /// Its removal is scheduled when the job commits.
    Remove,
    /// The request said to keep it.
    Keep,
    /// Other threads stay in it.
    InUse,
    /// The caller is not allowed to remove it, or it is only a directory on a machine.
    NotRemovable,
}

/// The environment a switch leaves, and what becomes of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LeftEnvironment {
    /// The environment.
    pub environment: EnvironmentRef,
    /// What becomes of it.
    pub action: LeftAction,
    /// How many of this thread's processes there are, ended unless told otherwise.
    pub processes: i64,
}

/// What a switch leaves behind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForkPlanLeaves {
    /// Whether an old session is closed.
    pub session: bool,
    /// The old environment, if it is one.
    pub environment: Option<LeftEnvironment>,
}

/// Something that stops a plan from being carried out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanBlocker {
    /// A stable code.
    pub code: String,
    /// What is wrong, in a sentence.
    pub message: String,
    /// What the caller lacks, where that is the reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing: Option<Vec<MissingAccess>>,
}

/// What a fork or a switch would do, before anything does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ForkPlan {
    /// The plan's identifier, which carries it out.
    pub preflight_id: Id,
    /// When it goes out of date.
    pub expires_at: Time,
    /// The thread it is made from.
    pub source: NodeRef,
    /// A thread of its own, or a switch.
    pub result: ForkResult,
    /// What the source runs on now.
    pub from: ForkPlanFrom,
    /// What the result will run on.
    pub to: ForkPlanTo,
    /// How the history travels.
    pub history: ForkPlanHistory,
    /// The files copied; `null` when the environment is shared or used as it
    /// is and nothing is copied.
    pub files: Option<FilesPlan>,
    /// The own-worktree choice, when the files and index are independent.
    #[serde(default)]
    pub worktree: Option<ForkPlanWorktree>,
    /// The exact saved files explicitly chosen, with their original availability and expiry.
    #[serde(default)]
    pub files_at: Option<crate::workspace::Snapshot>,
    /// Whether the agent's notes of the source directory go along; `null` when
    /// the directory is the same or has none.
    pub memory: Option<ForkPlanMemory>,
    /// What happens to each mount.
    pub mounts: Vec<PlanMount>,
    /// What becomes of each MCP server of the source's environment and of the target's.
    pub mcp: Vec<PlanMcpServer>,
    /// The values that will apply.
    pub options: AppliedChangeOptions,
    /// The page the result will have.
    pub access: ForkPlanAccess,
    /// Whose the result is.
    pub owner: ForkPlanOwner,
    /// What the result needs on its new side to be talked to, and whether the people on its page
    /// have it: the model on the target provider, for instance. The wizard offers to add it in the
    /// same step.
    pub depends: Vec<Dependency>,
    /// The threads that stand still from confirmation on; none for a fork.
    pub locks: Vec<NodeRef>,
    /// For a switch: what is left behind.
    pub leaves: Option<ForkPlanLeaves>,
    /// The time it takes, and for a switch the time the thread stands still.
    pub estimate: Estimate,
    /// Whether a turn is running at this moment; it is waited for or interrupted according to the
    /// options.
    pub turn_running: bool,
    /// What stops it; when there is any, it cannot be carried out.
    pub blockers: Vec<PlanBlocker>,
}

/// Why a thread was left out of a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// The caller may not switch it.
    NoPermission,
    /// It is archived.
    Archived,
    /// There is no such thread.
    NotFound,
}

/// A thread left out of a batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForkSkipped {
    /// The thread.
    pub thread: NodeRef,
    /// Why.
    pub reason: SkipReason,
}

/// What a batch switches the threads to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForkBatchTarget {
    /// The provider.
    pub provider: Option<ProviderRef>,
    /// The agent profile.
    pub agent: Option<AgentRef>,
    /// The model.
    pub model: Option<String>,
    /// The effort.
    pub effort: Option<String>,
}

/// Several threads switched together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ForkBatchPlan {
    /// The explicit reviewed notification choice for the whole batch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification: Option<crate::loss::OperationNotificationPlan>,
    /// The plan's identifier, which carries it out.
    pub preflight_id: Id,
    /// When it goes out of date.
    pub expires_at: Time,
    /// What they are switched to.
    pub to: ForkBatchTarget,
    /// One plan for every thread the caller may switch, each carrying its own blockers. With
    /// `all_of_environment`, threads the caller cannot see are planned as well: their `source` is
    /// redacted and their plan says only where they go.
    pub plans: Vec<ForkPlan>,
    /// Threads left out.
    pub skipped: Vec<ForkSkipped>,
    /// How many plans have no blockers.
    pub executable: i64,
    /// The values that will apply.
    pub options: AppliedChangeOptions,
    /// How long it takes.
    pub estimate: Estimate,
    /// How many turns are running.
    pub turns_running: i64,
}

pub use crate::workspace::{Job, JobError, JobPhase, JobProgress, PhaseState};

/// One job of a batch, with the thread it is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForkBatchJob {
    /// The thread.
    pub thread: NodeRef,
    /// Its job.
    pub job: Job,
}

/// How many jobs of a batch are in each state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForkBatchCounts {
    /// Waiting for their turn.
    pub queued: i64,
    /// Under way.
    pub running: i64,
    /// Finished well.
    pub succeeded: i64,
    /// Failed.
    pub failed: i64,
    /// Stopped.
    pub cancelled: i64,
}

/// A batch of switches being carried out, one job for each plan without blockers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForkBatch {
    /// The batch.
    pub batch_id: Id,
    /// The jobs, for the threads the caller may view.
    pub jobs: Vec<ForkBatchJob>,
    /// How far the batch has come.
    pub percent: u32,
    /// Jobs belonging to threads the caller may not view; they are counted but not listed.
    pub hidden: i64,
    /// How many jobs are in each state.
    pub counts: ForkBatchCounts,
}

/// What the thread of a moved environment does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MoveThreadAction {
    /// It goes on as it is.
    Continue,
    /// Its history is imported again on the other side.
    Reimport,
    /// It stops the move.
    Blocked,
}

/// A thread in an environment that is moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MovedThread {
    /// The thread; `null` when the caller may not view it. It is moved all the same.
    pub thread: Option<NodeRef>,
    /// What happens to it.
    pub action: MoveThreadAction,
    /// Whether a turn is running in it.
    pub turn_running: bool,
    /// A sentence about it.
    pub note: String,
}

/// What a move does with a mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MoveMountAction {
    /// It stays attached.
    Keep,
    /// It is detached.
    Detach,
}

/// What a move does with one mount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MovedMount {
    /// The mount.
    pub mount: MountRef,
    /// What happens to it.
    pub action: MoveMountAction,
    /// Where it will be, when it is kept.
    pub path: Option<String>,
    /// The mode on the target; null when detached.
    pub mode: Option<crate::mounts::MountMode>,
    /// Why it is detached; null when kept.
    pub detached_because: Option<MountDetachedBecause>,
}

/// The same environment going to another host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentMovePlan {
    /// The unique affected audience and the explicit confirmation choice.
    #[serde(default)]
    pub notification: crate::loss::OperationNotificationPlan,
    /// The plan's identifier, which carries it out.
    pub preflight_id: Id,
    /// When it goes out of date.
    pub expires_at: Time,
    /// The environment.
    pub environment: EnvironmentRef,
    /// Where it is.
    pub from: Option<HostRef>,
    /// The physical source machine, also present when it has no host page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_machine: Option<crate::resources::MachineRef>,
    /// The host that was asked for, or the one the scheduler would pick now.
    pub to: crate::environment::EnvironmentMovePlace,
    /// The files that are copied.
    pub files: FilesPlan,
    /// Actual and requested egress, verified before the destination becomes current.
    pub network: crate::environment::NetworkState,
    /// The threads in it.
    pub threads: Vec<MovedThread>,
    /// What happens to the mounts.
    pub mounts: Vec<PlanMount>,
    /// What becomes of each of its MCP servers there.
    pub mcp: Vec<PlanMcpServer>,
    /// Whether its processes can be suspended while the files are copied.
    pub held_still: bool,
    /// The values that will apply.
    pub options: AppliedChangeOptions,
    /// How long it takes.
    pub estimate: Estimate,
    /// What stops it.
    pub blockers: Vec<PlanBlocker>,
}

/// Why a unit is replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpdateReason {
    /// Updates are applied.
    Update,
    /// A person asked for a fresh unit.
    Recreate,
    /// Its template changed.
    Template,
    /// Its configuration changed.
    Configuration,
}

/// What a failed replacement brings back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Rollback {
    /// The previous physical source is lost and cannot be brought back.
    Unavailable,
    /// The old unit.
    Unit,
    /// The same unit with its old spec.
    Restart,
}

/// A thread that stands still while a unit is replaced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateThread {
    /// The thread; `null` when the caller may not view it.
    pub thread: Option<NodeRef>,
    /// Whether a turn is running in it.
    pub turn_running: bool,
}

/// A fresh unit for the same environment on the same host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentUpdatePlan {
    /// Exact immutable program builds selected for this unit's platform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_programs: Option<Vec<crate::admin::RunningHarnessBuild>>,
    /// People the optional update notice reaches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<UpdateAudience>,
    /// The plan's identifier, which carries it out.
    pub preflight_id: Id,
    /// When it goes out of date.
    pub expires_at: Time,
    /// The environment.
    pub environment: EnvironmentRef,
    /// Why.
    pub reason: UpdateReason,
    /// What will change; empty for a plain recreate.
    pub updates: Vec<PendingUpdate>,
    /// What stays, in words: the workspace, the home.
    pub kept: Vec<String>,
    /// What is lost, in words: running processes, files outside volumes.
    pub lost: Vec<String>,
    /// What a failure brings back.
    pub rollback: Rollback,
    /// Actual and requested egress; a failed restriction keeps an old open unit stopped.
    pub network: crate::environment::NetworkState,
    /// The threads in it.
    pub threads: Vec<UpdateThread>,
    /// The values that will apply.
    pub options: AppliedChangeOptions,
    /// How long it takes.
    pub estimate: Estimate,
    /// What stops it.
    pub blockers: Vec<PlanBlocker>,
}

/// The same plan, for a provider's runtime: the provider replaces the environment, and the threads
/// are the ones bound to that provider, every one of which stands still.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderUpdatePlan {
    /// Exact immutable program builds selected for this runtime's platform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_programs: Option<Vec<crate::admin::RunningHarnessBuild>>,
    /// People the optional update notice reaches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<UpdateAudience>,
    /// A lost runtime is made again rather than replaced on its old machine.
    #[serde(default)]
    pub recovery: bool,
    /// One explicit notification choice for this recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification: Option<crate::loss::OperationNotificationPlan>,
    /// Reviewed administrative page additions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub admin_grants: Vec<crate::loss::OperationAdminGrant>,
    /// The plan's identifier, which carries it out.
    pub preflight_id: Id,
    /// When it goes out of date.
    pub expires_at: Time,
    /// Why.
    pub reason: UpdateReason,
    /// What will change; empty for a plain recreate.
    pub updates: Vec<PendingUpdate>,
    /// What stays, in words.
    pub kept: Vec<String>,
    /// What is lost, in words.
    pub lost: Vec<String>,
    /// What a failure brings back.
    pub rollback: Rollback,
    /// Actual and requested egress of the provider's own runtime.
    pub network: crate::environment::NetworkState,
    /// The threads bound to the provider.
    pub threads: Vec<UpdateThread>,
    /// The values that will apply.
    pub options: AppliedChangeOptions,
    /// How long it takes.
    pub estimate: Estimate,
    /// What stops it.
    pub blockers: Vec<PlanBlocker>,
    /// The provider.
    pub provider: ProviderRef,
}

/// A fork with nothing to choose: a thread of its own in the same topic and the same environment, on the same
/// provider, profile and model, with all of the history or the history up to a turn. Nothing about the
/// environment, its home or its mounts changes; a fork that changes where or on what it runs is planned first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForkNowBody {
    /// Its name; the source's with a mark when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The last turn the history takes, from a message's "Fork from here"; all of it when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_turn: Option<i64>,
}

/// What a fork with nothing to choose started: the job, and the thread it makes, which exists once the job has
/// made it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForkNowReply {
    /// The job.
    pub job: crate::workspace::Job,
    /// The thread it makes.
    pub thread_id: Id,
}

crate::endpoints! {
    /// Forks a thread at once, with nothing to choose and no plan to review: the same topic, environment, provider, profile and model, with all of the history.
    ThreadForkNow: POST "/threads/{id}/forks/now", Rule("the same as planning a fork of it"), none, json ForkNowBody, json ForkNowReply, 202;
    /// Plans a fork or a switch of a thread, before anything is done.
    ThreadForkPreflight: POST "/threads/{id}/forks/preflight", Rule("what the result needs: for a new thread, manage or fork on the source and what making it takes; for a switch, what the request changes"), none, json ForkRequest, json ForkPlan, 200;
    /// Carries out a plan of a fork or a switch of a thread as a job; a switch locks the thread at once.
    ThreadForkCreate: POST "/threads/{id}/forks", Rule("the same as planning it"), none, json ForkCreateBody, json Job, 202;
    /// Plans a switch of several threads together.
    ForkBatchPreflight: POST "/forks/preflight", Rule("what a switch needs, on each thread planned"), none, json ForkBatchPreflightBody, json ForkBatchPlan, 200;
    /// Carries out a plan of a batch: one job for each plan without blockers.
    ForkBatchCreate: POST "/forks", Rule("as for each plan"), none, json ForkCreateBody, json ForkBatch, 202;
    /// Reads where a batch stands, limited to the threads the caller may view.
    ForkBatchGet: GET "/forks/{batch_id}", Rule("the batch's creator, or thread.manage on a listed thread"), none, none, json ForkBatch, 200;
    /// Cancels the jobs of a batch that have not passed their commit phase.
    ForkBatchCancel: POST "/forks/{batch_id}/cancel", Rule("the batch's creator"), none, none, json ForkBatch, 200;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_switch_names_only_what_it_changes_and_the_rest_stays() {
        let request: ForkRequest = serde_json::from_value(json!({
            "result": "replace",
            "to": { "provider_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "effort": "high" },
            "environment": { "same": true },
            "options": { "active_turn": "interrupt" }
        }))
        .unwrap();
        assert_eq!(request.result, ForkResult::Replace);
        assert_eq!(request.to.as_ref().and_then(|to| to.agent_id), None);
        assert!(matches!(
            request.environment,
            Some(ForkEnvironment::Same { same: true })
        ));
        assert_eq!(
            request.options.and_then(|options| options.active_turn),
            Some(ActiveTurn::Interrupt)
        );
    }

    #[test]
    fn the_environment_of_a_fork_is_the_same_one_another_or_a_new_one() {
        let existing: ForkEnvironment =
            serde_json::from_value(json!({ "use": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "files": { "mode": "selection", "include": ["src/**"] } })).unwrap();
        assert!(matches!(
            existing,
            ForkEnvironment::Existing {
                files: Some(FileSelection::Selection { .. }),
                ..
            }
        ));
        let made: ForkEnvironment = serde_json::from_value(json!({ "new": { "name": "Trial", "files": { "mode": "none" }, "private_views": false } })).unwrap();
        assert!(matches!(
            made,
            ForkEnvironment::New {
                new: ForkNewEnvironment {
                    private_views: Some(false),
                    ..
                }
            }
        ));
        assert!(
            serde_json::from_value::<ForkEnvironment>(
                json!({ "same": true, "files": { "mode": "all" } })
            )
            .is_err()
        );
    }

    #[test]
    fn a_request_says_what_becomes_of_each_mount_and_where_a_copy_goes_into_data() {
        let request: ForkRequest = serde_json::from_value(json!({
            "result": "new",
            "environment": { "new": { "name": "Trial" } },
            "mounts": [
                { "mount_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "fate": "detach" },
                { "mount_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6c", "fate": "copy", "into": "new_location" }
            ]
        }))
        .unwrap();
        let mounts = request.mounts.unwrap();
        assert_eq!(mounts[0].fate, MountFate::Detach);
        assert_eq!(mounts[0].into, None);
        assert_eq!(mounts[1].into, Some(CopyInto::NewLocation));
        assert!(
            serde_json::from_value::<MountChoice>(json!({
                "mount_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "fate": "copy", "into": "merge"
            }))
            .is_err(),
            "a copy into data replaces it or keeps both"
        );
    }

    #[test]
    fn content_is_checked_only_when_the_request_explicitly_asks() {
        for checked in [None, Some(false), Some(true)] {
            let mut value = json!({"result":"new"});
            if let Some(checked) = checked {
                value["check_fingerprints"] = json!(checked);
            }
            let request: ForkRequest = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(request.check_fingerprints, checked);
            assert_eq!(serde_json::to_value(&request).unwrap(), value);
            let mut movement = value;
            movement.as_object_mut().unwrap().remove("result");
            let request: crate::environment::EnvironmentMovePreflightBody =
                serde_json::from_value(movement.clone()).unwrap();
            assert_eq!(request.check_fingerprints, checked);
            assert_eq!(serde_json::to_value(&request).unwrap(), movement);
        }
        assert!(
            serde_json::from_value::<ForkRequest>(
                json!({"result":"new","check_fingerprints":"true"})
            )
            .is_err()
        );
        let id = orch_types::new_id();
        let written = json!({"result":"new","check_fingerprints":true,"fingerprint_request_id":id});
        let request: ForkRequest = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(request.fingerprint_request_id, Some(id));
        assert_eq!(serde_json::to_value(&request).unwrap(), written);
        let written = json!({"check_fingerprints":true,"fingerprint_request_id":id});
        let movement: crate::environment::EnvironmentMovePreflightBody =
            serde_json::from_value(written.clone()).unwrap();
        assert_eq!(movement.fingerprint_request_id, Some(id));
        assert_eq!(serde_json::to_value(movement).unwrap(), written);
    }

    #[test]
    fn a_plan_mount_says_its_fate_its_default_what_may_be_chosen_and_the_copy() {
        let written = json!({
            "mount": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "name": "datasets", "address": "acme.datasets" },
            "action": "keep", "path": "/mnt/datasets", "mode": "read_only", "detached_because": null,
            "fate": "copy", "default_fate": "as_is", "fates": ["detach", "as_is", "copy"], "copy_unavailable": null,
            "copy": {
                "measured": true, "file_count": 3, "total_bytes": 8192,
                "location": { "made": true, "kind": "volume", "source": "orch-mount-x-1", "storage": null },
                "holds": { "measured": true, "file_count": 2, "total_bytes": 2048 },
                "into": "new_location", "replace": true
            },
            "data_kind": "synced", "fingerprints_available": false, "fingerprints": null
        });
        let mount: PlanMount = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(mount.default_fate, MountFate::AsIs);
        assert_eq!(
            mount.copy.as_ref().unwrap().into,
            Some(CopyInto::NewLocation)
        );
        assert_eq!(serde_json::to_value(&mount).unwrap(), written);
        let server: PlanMcpServer = serde_json::from_value(json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6d", "name": "local-db", "own": false, "imported": true,
            "fate": "lost", "reason": "its imported definition belongs to another machine"
        }))
        .unwrap();
        assert_eq!(server.fate, McpFate::Lost);
    }

    #[test]
    fn a_plan_that_cannot_be_carried_out_says_what_the_caller_lacks() {
        let blocker: PlanBlocker = serde_json::from_value(
            json!({ "code": "thread.not_idle", "message": "A turn is running." }),
        )
        .unwrap();
        assert_eq!(blocker.missing, None);
        assert_eq!(
            serde_json::to_value(&blocker).unwrap(),
            json!({ "code": "thread.not_idle", "message": "A turn is running." })
        );
    }

    #[test]
    fn a_place_is_a_host_or_an_environment_by_its_kind() {
        let place: Place = serde_json::from_value(json!({
            "kind": "environment",
            "environment": {
                "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "name": "Main",
                "owner": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6c", "kind": "human", "username": "bo", "display_name": "Bo", "avatar": { "text": "B", "color": 2 } },
                "address": "acme.bo.main", "ports_exposed": false, "placement": "connected", "status": "offline"
            },
            "host": null, "machine": null,
            "facts": { "os": "linux", "cpus": 8, "memory_mb": null },
            "threads": 2, "topics": [], "topics_hidden": 1, "mounts": [],
            "usable": false, "reasons": ["offline"], "current": true
        }))
        .unwrap();
        assert!(matches!(
            place,
            Place::Environment(PlaceEnvironment { usable: false, .. })
        ));
    }
}

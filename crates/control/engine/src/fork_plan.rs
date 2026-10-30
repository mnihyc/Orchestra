//! What a fork or a switch would do, worked out before anything does it: who will own the result, how the history travels and what of it is kept, where the files and the agent's notes go, what stands still and what is left behind, how long it takes, and everything that stops it.
//!
//! A plan changes nothing and is kept for fifteen minutes ([`PREFLIGHT_VALID_MINUTES`]). It is made
//! in two steps so that the parts of it that are the API's to say (the page the result will have, what
//! the people on that page lack and which of the caller's lacks have names) can be added by the layer
//! that knows people: [`Engine::draft_fork`] works the plan out and [`Engine::keep_fork_plan`] keeps it
//! under an identifier. Carrying a plan out starts from the kept plan, and works it out again first
//! (`Engine::confirm_fork`): a plan that was good a minute ago is not carried out on what has changed
//! since.
//!
//! **Blockers, not refusals.** A caller who lacks something gets a plan that says so, with what they
//! lack, and cannot carry it out; the only questions answered with an error are those that have no
//! plan: no such thread, a thread nobody may see, a request that asks for what cannot be asked.
//!
//! **What a plan is made of**, in the order it is made: the thread and what it runs on now; what the
//! result runs on (each member the request leaves out stays as the source has it, and a provider
//! change that leaves the profile out takes the profile that can be bound there); where it lands; the
//! history, by the first method of the table that the adapters accept; the files and the notes, from
//! the list of the source's tree that was kept; what the caller needs on the new side, asked of the
//! access engine for each thing that changes and for nothing else; the estimate; and, for a switch,
//! what is left behind.

use crate::access::{Answer, ProviderUse};
use crate::binding::{self, Facts};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::harnesses;
use crate::jobs::estimates::{self, Making, Rate, Rates, Work};
use crate::resolved::{self, Resolved};
use crate::sessions::convert;
use chrono::{DateTime, Duration, Utc};
use orch_access_contract::decisions::{Decision, MissingAccess as Missing};
use orch_access_contract::permissions::Permission;
use orch_agent_contract::arrangement::{Arrangement, PlaceSide};
use orch_agent_contract::descriptor::{
    AgentDescriptor, ApprovalMode, ArrangementType as AgentArrangement,
};
use orch_agent_contract::facts::ExecutorFact;
use orch_agent_contract::history::{
    HistoryMethod as AgentMethod, HistoryMode as AgentMode, HistoryRequest, choose_method,
};
use orch_api_contract::binding::{
    CredentialHolder, CredentialPath as ApiCredentialPath, HarnessPlace, Isolation,
    IsolationOrchTools, IsolationTools, Metering as ApiMetering,
    ModelEnforcement as ApiModelEnforcement,
};
use orch_api_contract::forks::{
    ActiveTurn, AppliedChangeOptions, ChangeOptions, Estimate, FileSelection, FilesHome,
    FilesMissing, FilesPlan, FilesSource, ForkEnvironment, ForkPlan, ForkPlanAccess, ForkPlanFrom,
    ForkPlanHistory, ForkPlanLeaves, ForkPlanMemory, ForkPlanOwner, ForkPlanTarget, ForkPlanTo,
    ForkRequest, ForkResult, HistoryMethod, HistoryMode, LeftAction, LeftEnvironment,
    PREFLIGHT_VALID_MINUTES, PathMap, PlanBlocker, PlannedEnvironment,
};
use orch_api_contract::nodes::{EnvironmentRef, NodeKind, NodeRef};
use orch_api_contract::threads::{AgentRef, ProviderRef};
use orch_api_contract::users::UserRef;
use orch_authz::{Act, Caller, Principal, nodes};
use orch_db::Conn;
use orch_db::queries::environments::thread_engine::{self as environments, EnvironmentRow};
use orch_db::queries::jobs::forks as fork_rows;
use orch_db::queries::resources::thread_engine::{self as resources, AgentRow, ProviderRow};
use orch_db::queries::threads::{self, ThreadRow};
use orch_settings::{Place as SettingsPlace, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time, new_id};
use serde::{Deserialize, Serialize};

/// The words of the blockers a plan carries.
pub mod code {
    /// The caller lacks something the request needs; the blocker carries what.
    pub const ACCESS: &str = "access";
    /// The request changes nothing: model and effort alone are settings and need no switch.
    pub const NOTHING_TO_CHANGE: &str = "nothing_to_change";
    /// A fork, switch, move, recreate or update is already under way.
    pub const ALREADY_RUNNING: &str = "already_running";
    /// No arrangement is available and permitted.
    pub const NO_ARRANGEMENT: &str = "binding.incompatible";
    /// The model is not one the result may use.
    pub const MODEL_NOT_ALLOWED: &str = "model_not_allowed";
    /// The effort is not one the model lists.
    pub const EFFORT_NOT_LISTED: &str = "effort_not_listed";
    /// The target provider is not ready.
    pub const PROVIDER_NOT_READY: &str = "provider_not_ready";
    /// A switched-off provider cannot receive a new binding.
    pub const PROVIDER_DISABLED: &str = "provider_disabled";
    /// A switched-off environment cannot receive a new binding.
    pub const ENVIRONMENT_DISABLED: &str = "environment_disabled";
    /// The provider, the profile or the environment asked for is not there or is switched off.
    pub const UNAVAILABLE: &str = "unavailable";
    /// No host takes the new environment.
    pub const NO_HOST: &str = "no_host";
    /// A host would, and blocks for want of room.
    pub const NO_CAPACITY: &str = "no_capacity";
    /// The target environment's removal is scheduled.
    pub const REMOVAL_SCHEDULED: &str = "removal_scheduled";
    /// The thread is bound to nothing yet.
    pub const UNBOUND: &str = "unbound";
}

/// The longest name a thread made by a fork may be given, as a thread's name is bounded.
const NAME_MAX_CHARS: usize = binding::NAME_MAX_CHARS;

/// The fewest minutes a switch may wait for a turn: it may be told not to wait at all.
const WAIT_MINUTES_MIN: i64 = 0;
/// The most minutes a switch may wait for a turn: a day.
const WAIT_MINUTES_MAX: i64 = 1_440;
/// The fewest seconds the health checks may be given.
const VERIFY_SECS_MIN: i64 = 10;
/// The most seconds the health checks may be given: an hour.
const VERIFY_SECS_MAX: i64 = 3_600;

/// Why a plan could not be made at all.
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    /// There is no such thread, or the caller may not see it.
    #[error("there is no such thread")]
    NotFound,
    /// The thread, or a place above it, is archived.
    #[error("the thread is archived")]
    Archived,
    /// A side thread is not switched, moved, shared or archived.
    #[error("a side thread does not take this")]
    Side,
    /// The request asks for what cannot be asked.
    #[error("{message}")]
    Invalid {
        /// The member of the request at fault.
        path: String,
        /// What is wrong.
        message: String,
    },
    /// The plan that was named is not there, or is not the caller's.
    #[error("there is no such plan")]
    NoSuchPlan,
    /// The plan went out of date.
    #[error("the plan is out of date")]
    Expired,
    /// The plan has blockers.
    #[error("the plan cannot be carried out")]
    Blocked(Box<Draft>),
    /// A batch has no plan that can be carried out.
    #[error("none of the plans can be carried out")]
    NothingExecutable,
    /// A fork, switch or move holds the thread already.
    #[error("another job holds the thread")]
    Held(Id),
    /// Something the engine stands on failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl PlanError {
    /// A request that asks for what cannot be asked, at `path`.
    pub(crate) fn invalid(path: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            path: path.to_owned(),
            message: message.into(),
        }
    }
}

impl From<orch_db::DbError> for PlanError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}

impl From<orch_authz::Error> for PlanError {
    fn from(error: orch_authz::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl From<orch_settings::Error> for PlanError {
    fn from(error: orch_settings::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl From<serde_json::Error> for PlanError {
    fn from(error: serde_json::Error) -> Self {
        Self::Internal(error.into())
    }
}

/// A plan that is worked out and not yet kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    /// What was asked, with the omitted members left omitted.
    pub request: ForkRequest,
    /// The plan; its identifier is nil until it is kept, and the parts that are the API's to say are
    /// empty.
    pub plan: ForkPlan,
    /// What the caller lacks, for each blocker that says so: the blocker's index and everything
    /// lacking. The layer that names objects turns these into `PlanBlocker::missing`.
    pub lacks: Vec<(usize, Vec<Missing>)>,
    /// Private copy instruction: repository metadata was measured for a linked worktree.
    pub copy_worktree: bool,
    /// Exact history/execution shown by preflight, never inferred from a tab or ordinal.
    pub(crate) source_scope: SourceScope,
    /// What the job does with the mounts of a new environment: those it detaches and the data it copies.
    pub mount_decisions: crate::mount_copies::MountDecisions,
}

/// What a fork's new environment would be, as far as its mounts and its MCP servers go.
struct Prospect {
    home: Id,
    own: Json,
    machine: Option<Id>,
    driver: Option<String>,
    storage: Option<String>,
    template_spec: Option<Json>,
}

impl Prospect {
    fn target<'a>(
        &'a self,
        owner: Id,
        planned: &'a [orch_db::queries::resources::mounts::LocationRow],
        detached: &'a [Id],
    ) -> crate::mounts::Target<'a> {
        crate::mounts::Target {
            environment: Id::nil(),
            home: self.home,
            own: &self.own,
            owner,
            machine: self.machine,
            driver: self.driver.as_deref(),
            workspace: "/workspace",
            home_path: Some("/home/agent"),
            storage: self.storage.clone(),
            planned,
            detached,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SourceScope {
    pub view: Id,
    pub execution_revision: i64,
    pub segment: Option<Id>,
    pub reach: Reach,
}

impl SourceScope {
    pub(crate) fn of(thread: &ThreadRow, reach: Reach) -> Self {
        Self {
            view: thread.active_continuation_id,
            execution_revision: thread.execution_revision,
            segment: thread.current_segment_id,
            reach,
        }
    }
    pub(crate) fn matches(&self, thread: &ThreadRow) -> bool {
        *self == Self::of(thread, self.reach)
    }
}

impl Draft {
    /// Whether the plan can be carried out.
    #[must_use]
    pub fn is_clear(&self) -> bool {
        self.plan.blockers.is_empty()
    }
}

/// A plan as it is kept between asking for it and carrying it out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredPlan {
    /// What was asked.
    pub request: ForkRequest,
    /// What was answered.
    pub plan: ForkPlan,
    /// The thread it is for.
    pub source: Id,
    /// Old plans without an actual scope must be refreshed before execution.
    #[serde(default)]
    pub(crate) source_scope: Option<SourceScope>,
}

/// One plan of a kept batch: the thread, what was asked of it, and how the caller reached it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredMember {
    /// The thread.
    pub thread: Id,
    /// What was asked of it.
    pub request: ForkRequest,
    /// How the caller reached it.
    pub reach: Reach,
    #[serde(default)]
    pub(crate) source_scope: Option<SourceScope>,
}

/// The plan of a batch as it is kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredBatch {
    /// What was asked.
    pub body: orch_api_contract::forks::ForkBatchPreflightBody,
    /// What was answered.
    pub plan: orch_api_contract::forks::ForkBatchPlan,
    /// What each plan is for, in the order of the plans.
    pub members: Vec<StoredMember>,
}

// ---------------------------------------------------------------- small helpers

/// A blocker with no names to resolve.
#[must_use]
pub fn blocker_of(code: &str, message: impl Into<String>) -> PlanBlocker {
    blocker(code, message)
}

fn blocker(code: &str, message: impl Into<String>) -> PlanBlocker {
    PlanBlocker {
        code: code.to_owned(),
        message: message.into(),
        missing: None,
    }
}

/// The reference the wire has for a thread.
fn thread_ref(row: &ThreadRow) -> NodeRef {
    NodeRef {
        thread_display: None,
        id: row.node_id,
        kind: NodeKind::Thread,
        name: Some(row.name.clone()),
        redacted: false,
    }
}

async fn agent_ref(conn: &mut Conn, agent: &AgentRow) -> Result<AgentRef, EngineError> {
    Ok(serde_json::from_value(
        crate::views::agent_ref_json(conn, agent).await?,
    )?)
}

async fn provider_ref(conn: &mut Conn, provider: &ProviderRow) -> Result<ProviderRef, EngineError> {
    Ok(serde_json::from_value(
        crate::views::provider_ref_json(conn, provider).await?,
    )?)
}

async fn environment_ref(
    conn: &mut Conn,
    environment: &EnvironmentRow,
    now: Time,
) -> Result<EnvironmentRef, EngineError> {
    Ok(serde_json::from_value(
        crate::views::environment_ref_json(conn, environment, now).await?,
    )?)
}

async fn user_ref(conn: &mut Conn, user: Id) -> Result<UserRef, EngineError> {
    Ok(serde_json::from_value(
        crate::views::user_ref_json(conn, user).await?,
    )?)
}

fn parse_arrangement(word: Option<&str>) -> Option<AgentArrangement> {
    word.and_then(|word| serde_json::from_value(Json::String(word.to_owned())).ok())
}

/// The part of a path that lies under `directory`, if it does: directories are whole path steps.
fn under<'a>(path: &'a str, directory: &str) -> Option<&'a str> {
    if directory.is_empty() {
        return Some(path);
    }
    path.strip_prefix(directory)
        .and_then(|rest| rest.strip_prefix('/'))
}

// ---------------------------------------------------------------- what the plan is made from

/// The blockers of a plan as they are found.
#[derive(Debug, Default)]
struct Blockers {
    list: Vec<PlanBlocker>,
    lacks: Vec<(usize, Vec<Missing>)>,
}

impl Blockers {
    fn add(&mut self, code: &str, message: impl Into<String>) {
        self.list.push(blocker(code, message));
    }

    /// The caller lacks something; what, thing by thing, is kept beside the blocker.
    fn lack(&mut self, message: impl Into<String>, missing: Vec<Missing>) {
        self.lacks.push((self.list.len(), missing));
        self.list.push(blocker(code::ACCESS, message));
    }
}

/// What the thread runs on now.
struct Source {
    thread: ThreadRow,
    agent: AgentRow,
    provider: ProviderRow,
    environment: EnvironmentRow,
    descriptor: AgentDescriptor,
    settings: Resolved,
    arrangement: Option<AgentArrangement>,
    model: Option<String>,
}

/// Where the result lands.
#[derive(Debug, Clone)]
enum Landing {
    /// The source's own environment.
    Same,
    /// Another that exists.
    Existing(Box<EnvironmentRow>),
    /// One to be made, with the name it will have, the host chosen for it, and what its template promises
    /// of it.
    New {
        name: String,
        host: Option<Box<crate::places::HostPlace>>,
        promised: Promised,
    },
}

/// What the template an environment is made from says of it before it exists: what its exec node will
/// offer, and the harness kinds it lets run beside its tools. The environment the thread leaves tells
/// nothing of it, and nothing is reported by the new one before it is up; an arrangement is judged for it
/// by these alone, and judged again, by what it reports, when the job has made it.
#[derive(Debug, Clone, Default)]
struct Promised {
    offers: Vec<String>,
    colocated: Vec<String>,
}

impl Promised {
    /// The place an agent of this descriptor would find there: its machine is not the provider's, an
    /// executor the agent works through is taken to be there in the agent's own build, offering the tool
    /// sets the agent can work with, and the agent can be beside the tools only where the template lets its
    /// kind.
    fn place(&self, descriptor: &AgentDescriptor) -> PlaceSide {
        let mut executors: Vec<ExecutorFact> = Vec::new();
        for entry in descriptor.separated() {
            let Some(kind) = &entry.executor else {
                continue;
            };
            if executors.iter().any(|known| &known.kind == kind) {
                continue;
            }
            executors.push(ExecutorFact {
                kind: kind.clone(),
                version: descriptor.harness.version.clone(),
                available: true,
                reason: None,
                sandbox_modes: Vec::new(),
                tool_sets: entry.tool_sets.clone(),
            });
        }
        PlaceSide {
            environment_id: Id::nil(),
            offers: self.offers.clone(),
            executors,
            can_run_harness: self.colocated.contains(&descriptor.kind),
            same_machine: false,
            harness_version: None,
        }
    }
}

/// What the result runs on.
struct Target {
    agent: AgentRow,
    provider: ProviderRow,
    descriptor: AgentDescriptor,
    model: String,
    effort: Option<String>,
    arrangement: Option<Arrangement>,
    landing: Landing,
    directory: String,
    topic: Id,
}

impl Target {
    fn environment_id(&self) -> Option<Id> {
        match &self.landing {
            Landing::Same | Landing::New { .. } => None,
            Landing::Existing(row) => Some(row.id),
        }
    }
}

/// Whether the caller sees the thread, and whether it is archived.
async fn visibility(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
) -> Result<(bool, bool), PlanError> {
    match nodes::check(
        conn,
        principal,
        thread,
        Permission::ThreadView,
        Act::Use,
        orch_authz::Policy::default(),
    )
    .await
    {
        Ok(Decision::Allowed { .. }) => Ok((true, false)),
        Ok(Decision::Refused(_)) | Err(orch_authz::Error::NotFound(_)) => Ok((false, false)),
        Err(orch_authz::Error::Archived { .. }) => Ok((true, true)),
        Err(error) => Err(error.into()),
    }
}

/// What the caller lacks of one permission on a node, if anything.
async fn node_lacks(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    permission: Permission,
) -> Result<Option<Vec<Missing>>, PlanError> {
    match nodes::check(
        conn,
        principal,
        node,
        permission,
        Act::Use,
        orch_authz::Policy::default(),
    )
    .await
    {
        Ok(Decision::Allowed { .. }) | Err(orch_authz::Error::Archived { .. }) => Ok(None),
        Ok(Decision::Refused(refusal)) => Ok(Some(refusal.missing)),
        Err(orch_authz::Error::NotFound(_)) => Ok(Some(Vec::new())),
        Err(error) => Err(error.into()),
    }
}

/// What `principal` lacks of `permission` on the environment `environment`: nothing when they hold it.
async fn environment_lacks(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    permission: Permission,
) -> Result<Option<Vec<Missing>>, PlanError> {
    let object = orch_access_contract::decisions::ObjectRef {
        kind: orch_access_contract::permissions::ObjectKind::Environment,
        id: environment,
    };
    match orch_authz::resources::check(
        conn,
        principal,
        object,
        permission,
        orch_authz::Policy::default(),
    )
    .await
    {
        Ok(Decision::Allowed { .. }) => Ok(None),
        Ok(Decision::Refused(refusal)) => Ok(Some(refusal.missing)),
        Err(orch_authz::Error::NotFound(_)) => Ok(Some(Vec::new())),
        Err(error) => Err(error.into()),
    }
}

/// The options a change runs with: what the request gives, else the settings of the thread.
pub(crate) async fn applied_options(
    conn: &mut Conn,
    thread: Id,
    given: Option<&ChangeOptions>,
) -> Result<AppliedChangeOptions, PlanError> {
    let at = SettingsPlace::Node(thread);
    let active = match given.and_then(|options| options.active_turn) {
        Some(active) => active,
        None => match setting::<String>(conn, at, keys::SWITCH_ACTIVE_TURN)
            .await?
            .as_deref()
        {
            Some("interrupt") => ActiveTurn::Interrupt,
            _ => ActiveTurn::Wait,
        },
    };
    let wait_minutes = match given.and_then(|options| options.wait_minutes) {
        Some(minutes) => minutes,
        None => setting::<i64>(conn, at, keys::SWITCH_WAIT_MINUTES)
            .await?
            .unwrap_or(DEFAULT_WAIT_MINUTES),
    };
    let verify_timeout_secs = match given.and_then(|options| options.verify_timeout_secs) {
        Some(seconds) => seconds,
        None => setting::<i64>(conn, at, keys::SWITCH_VERIFY_TIMEOUT_SECS)
            .await?
            .unwrap_or(DEFAULT_VERIFY_SECS),
    };
    let keep_previous = match given.and_then(|options| options.keep_previous) {
        Some(keep) => keep,
        None => setting::<bool>(conn, at, keys::SWITCH_KEEP_PREVIOUS)
            .await?
            .unwrap_or(false),
    };
    if !(WAIT_MINUTES_MIN..=WAIT_MINUTES_MAX).contains(&wait_minutes) {
        return Err(PlanError::invalid(
            "options.wait_minutes",
            format!("is between {WAIT_MINUTES_MIN} and {WAIT_MINUTES_MAX}"),
        ));
    }
    if !(VERIFY_SECS_MIN..=VERIFY_SECS_MAX).contains(&verify_timeout_secs) {
        return Err(PlanError::invalid(
            "options.verify_timeout_secs",
            format!("is between {VERIFY_SECS_MIN} and {VERIFY_SECS_MAX}"),
        ));
    }
    Ok(AppliedChangeOptions {
        active_turn: active,
        wait_minutes,
        verify_timeout_secs,
        keep_previous,
        stop_processes: given
            .and_then(|options| options.stop_processes)
            .unwrap_or(true),
    })
}

/// How long a switch waits for a turn that is running when nothing says: what the setting ships with.
const DEFAULT_WAIT_MINUTES: i64 = 10;
/// How long the health checks may take when nothing says: what the setting ships with.
const DEFAULT_VERIFY_SECS: i64 = 120;

/// The turn the request ends the history at, and the one it begins with: the last completed turn when it
/// names none, the first turn when it names none.
fn turn_range(
    request: &ForkRequest,
    last_completed: Option<i32>,
    mode: HistoryMode,
) -> Result<Option<(i32, i32)>, PlanError> {
    let Some(last) = last_completed else {
        return Ok(None);
    };
    let history = request.history.as_ref();
    let to = history
        .and_then(|history| history.to_turn)
        .map_or(Ok(last), |turn| {
            i32::try_from(turn)
                .map_err(|_| PlanError::invalid("history.to_turn", "is a turn number"))
        })?;
    let from = history
        .and_then(|history| history.from_turn)
        .map_or(Ok(1), |turn| {
            i32::try_from(turn)
                .map_err(|_| PlanError::invalid("history.from_turn", "is a turn number"))
        })?;
    if mode == HistoryMode::Range && from > to {
        return Err(PlanError::invalid(
            "history.from_turn",
            "is not after the last turn",
        ));
    }
    if to < 1 || to > last {
        return Err(PlanError::invalid(
            "history.to_turn",
            format!("names a turn that is not there: the thread has completed {last}"),
        ));
    }
    if from < 1 {
        return Err(PlanError::invalid("history.from_turn", "counts from 1"));
    }
    Ok(Some((from, to)))
}

/// What a switch or a fork carries of the conversation when its request says nothing: the place's
/// `thread.switch_history`, as natively as the new agent program allows (carry, as shipped), a summary, or none.
async fn default_history_mode(conn: &mut Conn, thread: Id) -> Result<HistoryMode, PlanError> {
    let word = Box::pin(orch_settings::get::<String>(
        conn,
        orch_settings::Place::Node(thread),
        orch_settings_contract::registry::keys::THREAD_SWITCH_HISTORY,
    ))
    .await?;
    Ok(match word.as_deref() {
        Some("summary") => HistoryMode::Summary,
        Some("none") => HistoryMode::Nothing,
        _ => HistoryMode::All,
    })
}

// ---------------------------------------------------------------- the target

/// Where a new environment would be made: the host the scheduler or the request chooses, or why none; and
/// what its template promises of it, which decides how an agent can be placed in it.
async fn choose_host(
    conn: &mut Conn,
    principal: &Principal,
    new: &orch_api_contract::forks::ForkNewEnvironment,
    topic: Id,
    now: DateTime<Utc>,
    blockers: &mut Blockers,
) -> Result<(Option<Box<crate::places::HostPlace>>, Promised), PlanError> {
    use crate::places::{self, Ask, IDLE_MINUTES_DEFAULT};
    use orch_db::queries::resources::hosting as host_rows;
    let home = new.home_node_id.unwrap_or(topic);
    let template_id: Option<Id> = match new.template_id {
        Some(id) => Some(id),
        None => setting(conn, SettingsPlace::Node(home), keys::ENV_TEMPLATE_ID).await?,
    };
    let Some(template_id) = template_id else {
        blockers.add(
            code::NO_HOST,
            "no template is named, and none is set for the place the environment is made in",
        );
        return Ok((None, Promised::default()));
    };
    let template = match host_rows::template(conn, template_id).await? {
        Some(template)
            if template.deleted_at.is_none()
                && template.enabled
                && template.kind == "environment" =>
        {
            template
        }
        _ => {
            blockers.add(
                code::NO_HOST,
                "the template is not there, is switched off, or does not make environments",
            );
            return Ok((None, Promised::default()));
        }
    };
    let names_at = |path: &[&str]| -> Vec<String> {
        path.iter()
            .try_fold(&template.spec, |at, member| at.get(member))
            .cloned()
            .and_then(|listed| serde_json::from_value(listed).ok())
            .unwrap_or_default()
    };
    let promised = Promised {
        offers: names_at(&["exec", "offers"]),
        colocated: orch_agent_programs::Programs::colocated_for(&template.spec),
    };
    let mut own = new
        .config
        .as_ref()
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    if new.template_id.is_some() {
        own.insert(
            keys::ENV_TEMPLATE_ID.to_owned(),
            Json::String(template_id.to_string()),
        );
    }
    if let Some(Some(host)) = new.host_id {
        own.insert(keys::ENV_HOST_ID.to_owned(), Json::String(host.to_string()));
    }
    let own = Json::Object(own);
    let wanted = places::wanted(conn, home, template, &own).await?;
    let named: Option<Id> = match new.host_id {
        Some(Some(host)) => Some(host),
        _ => places::resolved_for_new(conn, home, &own, keys::ENV_HOST_ID)
            .await?
            .and_then(|value| serde_json::from_value(value).ok()),
    };
    let minutes_idle = places::resolved_for_new(conn, home, &own, keys::ENV_RECYCLE_IDLE_MINUTES)
        .await?
        .and_then(|value| value.as_i64())
        .unwrap_or(IDLE_MINUTES_DEFAULT);
    let listed = places::host_places(
        conn,
        principal,
        &Ask {
            home,
            wanted: &wanted,
            current: None,
            minutes_idle,
            now,
        },
    )
    .await?;
    match places::choose(&listed, named) {
        Ok(place) => Ok((Some(Box::new(place.clone())), promised)),
        Err(refused) => {
            blockers.add(refused.code, refused.message);
            Ok((None, promised))
        }
    }
}

/// The agent profile the result runs: the one the request names, else the source's where it can be
/// bound to the provider, else the profile that holds a subscription's login, else the place's default.
#[allow(
    clippy::too_many_arguments,
    reason = "everything the choice of a profile depends on, which is read once by the caller"
)]
async fn choose_agent(
    conn: &mut Conn,
    source: &Source,
    request: &ForkRequest,
    provider: &ProviderRow,
    judged_in: &EnvironmentRow,
    promised: Option<&Promised>,
    entry: Option<Vec<AgentArrangement>>,
    settings: &Resolved,
    blockers: &mut Blockers,
) -> Result<(AgentRow, AgentDescriptor), PlanError> {
    let named = request.to.as_ref().and_then(|to| to.agent_id);
    let mut candidates: Vec<Id> = Vec::new();
    match named {
        Some(id) => candidates.push(id),
        None => {
            candidates.push(source.agent.id);
            candidates.extend(provider.agent_id);
            if let Some(default) = setting::<Id>(
                conn,
                SettingsPlace::Node(source.thread.node_id),
                keys::AGENT_DEFAULT_AGENT_ID,
            )
            .await?
            {
                candidates.push(default);
            }
        }
    }
    candidates.dedup();
    let mut first: Option<(AgentRow, AgentDescriptor)> = None;
    for id in candidates {
        let Some(agent) = resources::agent(conn, id)
            .await?
            .filter(|agent| agent.deleted_at.is_none() && !agent.disabled)
        else {
            continue;
        };
        let Some(descriptor) = harnesses::current_descriptor(conn, &agent).await? else {
            continue;
        };
        let offers = binding::environment_offers(conn, judged_in).await?;
        let machine = binding::machine_arrangements_of(conn, provider).await?;
        let facts = Facts {
            descriptor: &descriptor,
            provider,
            environment: judged_in,
            offers: promised.map_or(offers.offers, |promised| promised.offers.clone()),
            settings,
            entry: entry.clone(),
            colocated: promised.map_or(offers.colocated, |promised| promised.colocated.clone()),
            runtime: binding::runtime_facts(conn, &agent, provider, judged_in).await?,
        };
        let fits = match promised {
            Some(promised) => {
                binding::choose_placed(&facts, &promised.place(&descriptor), machine, None)
            }
            None => binding::choose_with_machine(&facts, machine, None),
        }
        .is_ok();
        if fits {
            return Ok((agent, descriptor));
        }
        if first.is_none() {
            first = Some((agent, descriptor));
        }
    }
    match first {
        Some(found) => Ok(found),
        None => {
            blockers.add(
                code::UNAVAILABLE,
                "the agent profile is not there, is switched off, or has not reported what its harness is",
            );
            Ok((source.agent.clone(), source.descriptor.clone()))
        }
    }
}

// ---------------------------------------------------------------- making the plan

/// How the caller reaches the thread that is planned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reach {
    /// They see it, and need what the request needs on it.
    Seen,
    /// They manage the environment it is in and name all its threads together: they need not see it,
    /// and nothing is asked of the thread but that.
    Environment,
}

/// Which files go along, as the request puts it, and where.
#[derive(Debug, Clone, Default)]
struct Carry {
    selection: Option<FileSelection>,
    home: bool,
}

fn carry_of(request: &ForkRequest) -> Carry {
    match &request.environment {
        Some(ForkEnvironment::Existing { files, .. }) => Carry {
            selection: files.clone(),
            ..Carry::default()
        },
        Some(ForkEnvironment::New { new }) => Carry {
            selection: new.files.clone(),
            home: new.home.unwrap_or(false),
        },
        _ => Carry::default(),
    }
}

/// The files of the source directory that go along, and the notes among them, from the list of the
/// source's tree that was kept last.
struct Listing {
    as_of: DateTime<Utc>,
    files: i64,
    bytes: i64,
    without_content: i64,
    bytes_without_content: i64,
    sample: Vec<String>,
    notes_files: i64,
    notes_bytes: i64,
}

/// How many paths of files that cannot be copied a plan names.
pub(crate) const MISSING_SAMPLE: usize = 5;

fn listing_from_manifest(
    manifest: &crate::manifest::Manifest,
    directory: &str,
    selection: Option<&FileSelection>,
) -> Result<Listing, EngineError> {
    let choice = match selection {
        Some(FileSelection::Selection { include, exclude }) => {
            orch_link_contract::commands::Selection {
                include: include.clone(),
                exclude: exclude.clone().unwrap_or_default(),
            }
        }
        _ => orch_link_contract::commands::Selection::default(),
    };
    let matcher = crate::selection::Matcher::new(&choice)?;
    let mut listing = Listing {
        as_of: manifest.header().generated_at.get(),
        files: 0,
        bytes: 0,
        without_content: 0,
        bytes_without_content: 0,
        sample: Vec::new(),
        notes_files: 0,
        notes_bytes: 0,
    };
    for entry in manifest
        .entries()
        .iter()
        .filter(|entry| entry.kind == crate::manifest::EntryKind::File)
    {
        let Some(relative) = under(&entry.path, directory) else {
            continue;
        };
        if crate::memory::is_note(relative) {
            listing.notes_files += 1;
            listing.notes_bytes += entry.size;
        }
        if matches!(selection, Some(FileSelection::Nothing)) || !matcher.matches(relative) {
            continue;
        }
        listing.files += 1;
        listing.bytes = listing.bytes.saturating_add(entry.size);
        if entry.hash.is_none() {
            listing.without_content += 1;
            listing.bytes_without_content =
                listing.bytes_without_content.saturating_add(entry.size);
            if listing.sample.len() < MISSING_SAMPLE {
                listing.sample.push(relative.to_owned());
            }
        }
    }
    Ok(listing)
}

impl Engine {
    /// Counts the files of a thread's directory from the last list of its environment's tree.
    async fn list_directory(
        &self,
        conn: &mut Conn,
        environment: Id,
        directory: &str,
        selection: Option<&FileSelection>,
        now: DateTime<Utc>,
    ) -> Result<Listing, EngineError> {
        let listing = Listing {
            as_of: now,
            files: 0,
            bytes: 0,
            without_content: 0,
            bytes_without_content: 0,
            sample: Vec::new(),
            notes_files: 0,
            notes_bytes: 0,
        };
        let Some(row) =
            orch_db::queries::environments::workspace::environment(conn, environment).await?
        else {
            return Ok(listing);
        };
        let (Some(blob), true) = (row.manifest_blob, self.blobs().is_some()) else {
            return Ok(listing);
        };
        let manifest = match self.read_manifest(&blob).await {
            Ok(manifest) => manifest,
            Err(error) => {
                tracing::warn!(%error, %environment, "the list of an environment's tree could not be read for a plan");
                return Ok(listing);
            }
        };
        listing_from_manifest(&manifest, directory, selection)
    }

    async fn list_selected_live(
        &self,
        conn: &mut Conn,
        environment: &EnvironmentRow,
        directory: &str,
        selection: &FileSelection,
    ) -> Result<Listing, EngineError> {
        let FileSelection::Selection { include, exclude } = selection else {
            return Err(EngineError::unreadable(
                "a live tree needs an explicit selection",
            ));
        };
        let qualify = |path: &String| {
            if directory.is_empty() {
                path.clone()
            } else {
                format!("{directory}/{path}")
            }
        };
        let selection = orch_link_contract::commands::Selection {
            include: include.iter().map(qualify).collect(),
            exclude: exclude
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(qualify)
                .collect(),
        };
        let _matcher = crate::selection::Matcher::new(&selection)?;
        let machine = environment
            .machine_id
            .ok_or_else(|| EngineError::unreadable("the source has no connector"))?;
        let command = crate::commands::Outgoing::new(
            new_id(),
            machine,
            orch_link_contract::commands::SerialKey::Environment(environment.id),
            &orch_link_contract::commands::FsManifest {
                environment_id: environment.id,
                selection,
            },
        )?;
        let Some(crate::commands::Outcome::Succeeded(value)) = self.ask_bounded(command).await?
        else {
            return Err(EngineError::unreadable(
                "the source did not list the selected paths",
            ));
        };
        let result: orch_link_contract::commands::FsManifestResult = serde_json::from_value(value)?;
        let blob = crate::selection::rebase_manifest(
            self,
            conn,
            &result.manifest_blob,
            crate::sessions::workspace_root(
                &environment.workspace_root,
                &environment.facts,
                environment.generation,
            ),
        )
        .await?;
        let manifest = self.read_manifest(&blob).await?;
        listing_from_manifest(&manifest, directory, None)
    }
}

/// What the result runs on, as the wire shows the isolation of the arrangement.
fn isolation_view(
    option: &binding::ArrangementOption,
    provider: &ProviderRow,
    agent: &AgentRow,
    descriptor: &AgentDescriptor,
) -> Result<Isolation, EngineError> {
    use orch_agent_contract::descriptor::OrchToolsSupport;
    let isolation = binding::isolation(
        option,
        provider,
        provider.agent_id == Some(agent.id),
        descriptor.capabilities.usage_tokens,
    );
    let offered = descriptor.capabilities.orch_tools == OrchToolsSupport::Mcp;
    Ok(Isolation {
        kind: convert(&isolation.kind)?,
        harness_runs_in: if isolation.harness_with_provider {
            HarnessPlace::Provider
        } else {
            HarnessPlace::Environment
        },
        executor: isolation.executor.clone(),
        executor_ref: crate::programs::executor_ref(
            orch_agent_programs::Programs::builtin(),
            isolation.executor.as_deref(),
            isolation.tool_set.as_deref(),
        ),
        tool_set: isolation.tool_set.clone(),
        tools: if isolation.executor.is_some() {
            IsolationTools::Executor
        } else {
            IsolationTools::Own
        },
        orch_tools: IsolationOrchTools {
            offered,
            reason: (!offered).then_some(orch_api_contract::binding::OrchToolsReason::Harness),
        },
        credential_path: convert::<_, ApiCredentialPath>(&isolation.path)?,
        held_by: if isolation.handed_over {
            CredentialHolder::Environment
        } else {
            CredentialHolder::Provider
        },
        form: isolation.form,
        lifetime_secs: isolation.lifetime_secs,
        model_enforcement: convert::<_, ApiModelEnforcement>(&isolation.model_enforcement)?,
        metering: convert::<_, ApiMetering>(&isolation.metering)?,
    })
}

impl Engine {
    /// Works out what a fork or a switch of a thread would do for the caller, and changes nothing.
    pub async fn draft_fork(
        &self,
        caller: &Caller,
        thread: Id,
        request: &ForkRequest,
    ) -> Result<Draft, PlanError> {
        let mut conn = self.inner.db.acquire().await?;
        self.draft_in(&mut conn, caller, thread, request, Reach::Seen, None)
            .await
    }

    /// As [`Engine::draft_fork`], on a connection the caller holds and reaching the thread as `reach` says.
    /// `own_batch` is the batch being carried out, when there is one: the jobs it has made already are its
    /// own and do not stand in the way of the plans that follow.
    #[allow(
        clippy::too_many_lines,
        reason = "one plan is made from one reading of the database, in the order its parts depend on each other"
    )]
    pub(crate) async fn draft_in(
        &self,
        conn: &mut Conn,
        caller: &Caller,
        thread_id: Id,
        request: &ForkRequest,
        reach: Reach,
        own_batch: Option<Id>,
    ) -> Result<Draft, PlanError> {
        let now = self.inner.clock.now();
        let person = caller.user();
        let principal = Principal::load(&mut *conn, caller.clone(), Time::new(now)).await?;
        let thread = threads::thread(&mut *conn, thread_id)
            .await?
            .filter(|row| row.deleted_at.is_none())
            .ok_or(PlanError::NotFound)?;
        let (sees, archived) = visibility(&mut *conn, &principal, thread_id).await?;
        if !sees && reach == Reach::Seen {
            return Err(PlanError::NotFound);
        }
        if archived || thread.archived_at.is_some() {
            return Err(PlanError::Archived);
        }
        let replace = request.result == ForkResult::Replace;
        if replace && thread.side_of.is_some() {
            return Err(PlanError::Side);
        }
        let (Some(agent_id), Some(provider_id), Some(environment_id)) =
            (thread.agent_id, thread.provider_id, thread.environment_id)
        else {
            return Err(PlanError::invalid(
                "",
                "the thread is bound to nothing yet: bind it, there is nothing to carry over",
            ));
        };
        if matches!(
            request.environment,
            Some(ForkEnvironment::Same { same: false })
        ) {
            return Err(PlanError::invalid(
                "environment.same",
                "is true, or the member is left out",
            ));
        }
        if let Some(directory) = request.directory.as_deref()
            && !binding::directory_is_valid(directory)
        {
            return Err(PlanError::invalid(
                "directory",
                "is relative to the workspace root and does not go up",
            ));
        }
        if let Some(cwd) = request.cwd.as_deref()
            && !binding::directory_is_valid(cwd)
        {
            return Err(PlanError::invalid(
                "cwd",
                "is relative to the thread's directory and does not go up",
            ));
        }
        if let Some(name) = request.name.as_deref()
            && (name.trim().is_empty() || name.chars().count() > NAME_MAX_CHARS)
        {
            return Err(PlanError::invalid(
                "name",
                format!("is between 1 and {NAME_MAX_CHARS} characters"),
            ));
        }
        // A request that names no history takes what the place carries on a switch or a fork.
        let history = match request.history.clone() {
            Some(history) => history,
            None => orch_api_contract::forks::ForkHistory {
                mode: default_history_mode(&mut *conn, thread_id).await?,
                from_turn: None,
                to_turn: None,
            },
        };
        let options = applied_options(&mut *conn, thread_id, request.options.as_ref()).await?;

        // ---- what the thread runs on now
        let agent = resources::agent(&mut *conn, agent_id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the thread's profile is gone"))?;
        let provider = resources::provider(&mut *conn, provider_id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the thread's provider is gone"))?;
        let environment = environments::environment(&mut *conn, environment_id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the thread's environment is gone"))?;
        let descriptor = harnesses::current_descriptor(&mut *conn, &agent)
            .await?
            .ok_or_else(|| EngineError::unreadable("the thread's profile never reported"))?;
        let settings = resolved::load(&mut *conn, thread_id).await?;
        let source = Source {
            arrangement: parse_arrangement(thread.arrangement.as_deref()),
            model: settings.model.clone(),
            thread,
            agent,
            provider,
            environment,
            descriptor,
            settings,
        };
        let mut blockers = Blockers::default();
        let target_topic = request.topic_id.unwrap_or(source.thread.topic_id);

        // ---- what the result runs on
        let wanted_provider = request.to.as_ref().and_then(|to| to.provider_id);
        let provider = match wanted_provider {
            Some(id) if id != source.provider.id => {
                match resources::provider(&mut *conn, id)
                    .await?
                    .filter(|row| row.deleted_at.is_none())
                {
                    Some(row) => row,
                    None => {
                        blockers.add(code::UNAVAILABLE, "the provider is not there");
                        source.provider.clone()
                    }
                }
            }
            _ => source.provider.clone(),
        };
        let model = request
            .to
            .as_ref()
            .and_then(|to| to.model.clone())
            .or_else(|| source.model.clone());
        let destination = if target_topic == source.thread.topic_id {
            source.thread.node_id
        } else {
            target_topic
        };
        if let Some(why) =
            crate::provider_policy::forbidden(&mut *conn, destination, provider.id).await?
        {
            blockers.add("provider_not_allowed", why.to_string());
        }
        let asked_effort = request.to.as_ref().and_then(|to| to.effort.clone());
        let model_row = match model.as_deref() {
            Some(model) => resources::model(&mut *conn, provider.id, model).await?,
            None => None,
        };
        let model_name = model.clone().unwrap_or_default();
        match &model {
            None => blockers.add(
                code::MODEL_NOT_ALLOWED,
                "no model is chosen for the thread, and the request names none",
            ),
            Some(name) => {
                if crate::models::check_allowed(
                    model_row.as_ref(),
                    source.settings.models_allowed.as_deref(),
                    name,
                )
                .is_err()
                {
                    blockers.add(
                        code::MODEL_NOT_ALLOWED,
                        format!(
                            "the model {name} is not allowed on the provider {}",
                            provider.name
                        ),
                    );
                }
            }
        }
        // An effort the new model does not list falls back to that model's default, unless the request
        // asked for it by name.
        let effort = {
            let wanted = asked_effort
                .clone()
                .or_else(|| source.settings.effort.clone());
            match (&wanted, &model_row) {
                (Some(effort), Some(row)) => {
                    let parameters = binding::model_parameters(row)?;
                    if parameters.efforts().iter().any(|listed| listed == effort) {
                        wanted
                    } else if asked_effort.is_some() {
                        blockers.add(
                            code::EFFORT_NOT_LISTED,
                            format!("the model does not take the effort {effort}"),
                        );
                        wanted
                    } else {
                        parameters.default_effort.clone()
                    }
                }
                _ => wanted,
            }
        };
        if provider.status == "disabled" {
            blockers.add(code::PROVIDER_DISABLED, "the provider is switched off");
        } else if provider.status != "ready" {
            blockers.add(
                code::PROVIDER_NOT_READY,
                format!("the provider {} is {}", provider.name, provider.status),
            );
        }

        // ---- where it lands
        if let Some(worktree) = &request.worktree {
            if replace
                || !matches!(
                    request.environment,
                    None | Some(ForkEnvironment::Same { same: true })
                )
            {
                return Err(PlanError::invalid(
                    "worktree",
                    "an own worktree is for a new thread in the same environment",
                ));
            }
            if !binding::directory_is_valid(&worktree.directory)
                || worktree.directory.is_empty()
                || !binding::directory_is_valid(&worktree.repository_dir)
                || !orch_settings_contract::registry::is_git_branch(&worktree.branch)
                || request
                    .directory
                    .as_ref()
                    .is_some_and(|directory| directory != &worktree.directory)
            {
                return Err(PlanError::invalid(
                    "worktree",
                    "choose an unused relative directory and a valid new branch",
                ));
            }
            if request.files_at.is_some()
                && matches!(
                    worktree.from,
                    orch_api_contract::source::WorktreeFrom::Commit { .. }
                )
            {
                return Err(PlanError::invalid(
                    "files_at",
                    "saved local files and From commit are alternatives",
                ));
            }
        }
        if request.files_at.is_some()
            && request.worktree.is_none()
            && request.directory.as_deref().is_none_or(str::is_empty)
        {
            return Err(PlanError::invalid(
                "directory",
                "saved files need an unused directory",
            ));
        }
        let wanted_directory = request
            .worktree
            .as_ref()
            .map(|choice| choice.directory.clone())
            .or_else(|| request.directory.clone());
        let (landing, judged_in) = match &request.environment {
            None | Some(ForkEnvironment::Same { .. }) => {
                (Landing::Same, source.environment.clone())
            }
            Some(ForkEnvironment::Existing { environment_id, .. })
                if *environment_id == source.environment.id =>
            {
                (Landing::Same, source.environment.clone())
            }
            Some(ForkEnvironment::Existing { environment_id, .. }) => {
                match environments::environment(&mut *conn, *environment_id)
                    .await?
                    .filter(|row| row.deleted_at.is_none())
                {
                    Some(row) => {
                        if row.status == "disabled" {
                            blockers.add(
                                code::ENVIRONMENT_DISABLED,
                                "the environment is switched off",
                            );
                        }
                        if matches!(row.status.as_str(), "lost" | "removed") {
                            blockers.add(
                                code::UNAVAILABLE,
                                "The destination environment was lost or removed",
                            );
                        }
                        if row.remove_at.is_some() {
                            blockers.add(
                                code::REMOVAL_SCHEDULED,
                                format!("the removal of the environment {} is scheduled", row.name),
                            );
                        }
                        (Landing::Existing(Box::new(row.clone())), row)
                    }
                    None => {
                        blockers.add(code::UNAVAILABLE, "the environment is not there");
                        (Landing::Same, source.environment.clone())
                    }
                }
            }
            Some(ForkEnvironment::New { new }) => {
                let (host, promised) = choose_host(
                    &mut *conn,
                    &principal,
                    new,
                    target_topic,
                    now,
                    &mut blockers,
                )
                .await?;
                (
                    Landing::New {
                        name: new
                            .name
                            .clone()
                            .unwrap_or_else(|| crate::environments::DEFAULT_NAME.to_owned()),
                        host,
                        promised,
                    },
                    source.environment.clone(),
                )
            }
        };
        let directory = wanted_directory.unwrap_or_else(|| source.thread.workspace_dir.clone());

        // ---- the profile, the access to it, and the arrangement
        let mut entry: Option<Vec<AgentArrangement>> = None;
        let provider_changes = provider.id != source.provider.id;
        if replace && !provider_changes {
            // nothing is asked about a provider that stays
        } else if reach == Reach::Seen {
            if !caller.permits(Permission::ProviderUse) {
                blockers.add(
                    code::ACCESS,
                    "this credential does not include provider.use for the requested new binding",
                );
            }
            let node = if replace { thread_id } else { target_topic };
            let answer = self
                .inner
                .access
                .provider_use(
                    &mut *conn,
                    &ProviderUse {
                        person,
                        node,
                        provider: provider.id,
                        model: model.as_deref(),
                        effort: effort.as_deref(),
                    },
                )
                .await
                .map_err(EngineError::from)?;
            match answer {
                Answer::Allowed { arrangements } => entry = arrangements,
                Answer::Refused(missing) => blockers.lack(
                    format!(
                        "you may not use the provider {} with the model and effort asked",
                        provider.name
                    ),
                    missing,
                ),
            }
        }
        // An environment that is made for the result is judged by what its template promises of it, and not
        // by the one the thread leaves: whether the agent can be beside its tools there, and what carries
        // out its tool calls.
        let made_for_it = match &landing {
            Landing::New { promised, .. } => Some(promised.clone()),
            _ => None,
        };
        let (agent, descriptor) = choose_agent(
            &mut *conn,
            &source,
            request,
            &provider,
            &judged_in,
            made_for_it.as_ref(),
            entry.clone(),
            &source.settings,
            &mut blockers,
        )
        .await?;
        let approval_place = if replace { thread_id } else { target_topic };
        let target_settings = crate::resolved::load(&mut *conn, approval_place).await?;
        let approval_mode =
            match binding::offered_approval_mode(&descriptor, target_settings.approval_mode) {
                Ok(mode) => mode,
                Err(why) => {
                    blockers.add("approval_mode_unsupported", why);
                    target_settings.approval_mode
                }
            };
        if !target_settings.allowed_modes.contains(&approval_mode) {
            blockers.add(
                "approval_mode_not_allowed",
                crate::sessions::APPROVAL_NOT_ALLOWED,
            );
        }
        if approval_mode != target_settings.approval_mode {
            let setting = orch_settings::resolve(
                &mut *conn,
                SettingsPlace::Node(approval_place),
                keys::APPROVAL_MODE,
            )
            .await
            .map_err(EngineError::from)?;
            if !setting.open_to_write() || (!replace && setting.resolved.locked_by.is_some()) {
                blockers.add("approval_mode_locked", "The place locks an approval mode that the target program does not offer. Ask a manager to change the lock or choose another program.");
            }
        }
        let agent_changes = agent.id != source.agent.id;
        if (!replace || agent_changes) && reach == Reach::Seen {
            if !caller.permits(Permission::AgentUse) {
                blockers.add(
                    code::ACCESS,
                    "this credential does not include agent.use for the requested new binding",
                );
            }
            let node = if replace { thread_id } else { target_topic };
            if let Answer::Refused(missing) = self
                .inner
                .access
                .agent_use(&mut *conn, person, node, agent.id)
                .await
                .map_err(EngineError::from)?
            {
                blockers.lack(
                    format!("you may not use the agent profile {}", agent.name),
                    missing,
                );
            }
        }
        let offers = binding::environment_offers(&mut *conn, &judged_in).await?;
        let machine_list = binding::machine_arrangements_of(&mut *conn, &provider).await?;
        let place = made_for_it
            .as_ref()
            .map(|promised| promised.place(&descriptor));
        let facts = Facts {
            descriptor: &descriptor,
            provider: &provider,
            environment: &judged_in,
            offers: made_for_it
                .as_ref()
                .map_or(offers.offers, |promised| promised.offers.clone()),
            settings: &source.settings,
            entry,
            colocated: made_for_it.map_or(offers.colocated, |promised| promised.colocated),
            runtime: binding::runtime_facts(conn, &agent, &provider, &judged_in).await?,
        };
        let choose = |wanted: Option<AgentArrangement>| match &place {
            Some(place) => binding::choose_placed(&facts, place, machine_list.clone(), wanted),
            None => binding::choose_with_machine(&facts, machine_list.clone(), wanted),
        };
        let requested: Option<AgentArrangement> = request
            .to
            .as_ref()
            .and_then(|to| to.arrangement)
            .map(|word| convert(&word))
            .transpose()?;
        let chosen = match requested.or(source.arrangement) {
            Some(kind) => choose(Some(kind)).or_else(|first| {
                if requested.is_some() {
                    Err(first)
                } else {
                    choose(None)
                }
            }),
            None => choose(None),
        };
        let arrangement = match chosen {
            Ok(arrangement) => Some(arrangement),
            Err(why) => {
                blockers.add(
                    code::NO_ARRANGEMENT,
                    format!("no arrangement is available: {}", binding::sentence(&why)),
                );
                None
            }
        };
        let options_listed = match &place {
            Some(place) => binding::options_placed(&facts, place, machine_list),
            None => binding::options(&facts, machine_list),
        };

        let target = Target {
            agent,
            provider,
            descriptor,
            model: model_name,
            effort,
            arrangement,
            landing,
            directory,
            topic: target_topic,
        };

        // ---- what changes, and what the caller needs for it
        let target_environment = target.environment_id().unwrap_or(source.environment.id);
        let environment_changes =
            matches!(target.landing, Landing::Existing(_) | Landing::New { .. });
        let directory_changes = target.directory != source.thread.workspace_dir;
        let arrangement_changes = target
            .arrangement
            .as_ref()
            .is_some_and(|chosen| Some(chosen.kind) != source.arrangement);
        let changes_binding = provider_changes || agent_changes || arrangement_changes;
        if replace && !(changes_binding || environment_changes || directory_changes) {
            blockers.add(
                code::NOTHING_TO_CHANGE,
                "the request changes nothing: a model or an effort alone is a setting, and needs no switch",
            );
        }
        if reach == Reach::Seen {
            if replace {
                if changes_binding
                    && let Some(missing) =
                        node_lacks(&mut *conn, &principal, thread_id, Permission::ThreadManage)
                            .await?
                {
                    blockers.lack("you may not manage this thread", missing);
                }
            } else {
                // Either right to fork will do: managing the thread, or its own row of forking.
                if node_lacks(&mut *conn, &principal, thread_id, Permission::ThreadManage)
                    .await?
                    .is_some()
                    && let Some(fork) =
                        node_lacks(&mut *conn, &principal, thread_id, Permission::ThreadFork)
                            .await?
                {
                    blockers.lack("you may neither fork nor manage this thread", fork);
                }
                // The files that are taken are the environment's, and are read by whoever holds its
                // `files.view`, whatever they hold on the thread.
                if (environment_changes || directory_changes)
                    && let Some(missing) = environment_lacks(
                        &mut *conn,
                        &principal,
                        source.environment.id,
                        Permission::FilesView,
                    )
                    .await?
                {
                    blockers.lack("you may not read the files that would be taken", missing);
                }
                if let Some(missing) = node_lacks(
                    &mut *conn,
                    &principal,
                    target.topic,
                    Permission::TopicStartThreads,
                )
                .await?
                {
                    blockers.lack("you may not start a thread in that topic", missing);
                }
                // The page of another thread is carried over only by whoever manages that thread.
                if let Some(orch_api_contract::access::ThreadAccess::As(as_page)) = &request.access
                    && as_page.source != thread_id
                {
                    if threads::thread(&mut *conn, as_page.source)
                        .await?
                        .is_none_or(|row| row.deleted_at.is_some())
                    {
                        return Err(PlanError::invalid(
                            "access.as",
                            "is a thread whose page is carried over",
                        ));
                    }
                    if let Some(missing) = node_lacks(
                        &mut *conn,
                        &principal,
                        as_page.source,
                        Permission::ThreadManage,
                    )
                    .await?
                    {
                        blockers.lack("you may not carry over the page of that thread", missing);
                    }
                }
                if matches!(target.landing, Landing::Same)
                    && let Answer::Refused(missing) = self
                        .inner
                        .access
                        .environment_manage(&mut *conn, person, source.environment.id)
                        .await
                        .map_err(EngineError::from)?
                {
                    blockers.lack(
                        "putting another thread in this environment takes managing it",
                        missing,
                    );
                }
            }
        }
        if replace && (environment_changes || directory_changes)
            || (!replace && environment_changes)
        {
            let moving = orch_authz::environments::Moving {
                thread: replace.then_some(thread_id),
                from: replace.then_some(source.environment.id),
                to: target.environment_id(),
                all_of_the_environment: reach == Reach::Environment,
            };
            if (replace || matches!(target.landing, Landing::Existing(_)))
                && let Decision::Refused(refusal) = orch_authz::environments::check_move(
                    &mut *conn,
                    &principal,
                    &moving,
                    orch_authz::Policy::default(),
                )
                .await?
            {
                blockers.lack("you may not decide where this thread runs", refusal.missing);
            }
        }

        // ---- what is already going on
        for id in [source.environment.id, target_environment]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
        {
            if fork_rows::open_touching_environment(&mut *conn, id)
                .await?
                .iter()
                .any(|job| {
                    (job.node_id != Some(thread_id) || replace)
                        && own_batch.is_none_or(|own| job.batch_id != Some(own))
                })
                || fork_rows::environment_is_being_changed(&mut *conn, id)
                    .await?
                    .is_some()
            {
                blockers.add(
                    code::ALREADY_RUNNING,
                    "a fork, switch, move, recreate or update is already under way for an environment it touches",
                );
                break;
            }
        }
        if orch_db::queries::agent_history::removing(conn, thread_id).await?
            || crate::locks::held_by(&mut *conn, thread_id)
                .await?
                .is_some()
        {
            blockers.add(
                code::ALREADY_RUNNING,
                "a fork or a switch holds this thread already",
            );
        }

        self.assemble(
            conn,
            &principal,
            &source,
            &target,
            request,
            &history,
            options,
            &options_listed,
            approval_mode,
            blockers,
            reach,
            now,
        )
        .await
    }
}

// ---------------------------------------------------------------- the rest of the plan

/// What the thread's page depends on, as the wire has it.
async fn dependency_view(
    conn: &mut Conn,
    dependency: &orch_authz::depends::Dependency,
) -> Result<orch_api_contract::access::Dependency, EngineError> {
    use orch_api_contract::access::{Dependency, DependencyObject, DependencyOpen};
    use orch_authz::depends::Open;
    let owner = match dependency.owner {
        Some(owner) => Some(user_ref(conn, owner).await?),
        None => None,
    };
    let through = match dependency.through {
        Some(node) => {
            let name = threads::node_name(conn, node).await?.unwrap_or_default();
            Some(NodeRef {
                thread_display: None,
                id: node,
                kind: NodeKind::Topic,
                name: Some(name),
                redacted: false,
            })
        }
        None => None,
    };
    Ok(Dependency {
        for_permission: dependency.for_permission.as_str().to_owned(),
        object: DependencyObject {
            kind: dependency.object.kind.as_str().to_owned(),
            id: dependency.object.id,
            name: dependency.name.clone(),
            owner,
        },
        permission: dependency.permission.as_str().to_owned(),
        choice: None,
        options: dependency
            .options
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?,
        open: match dependency.open {
            Open::All => DependencyOpen::All,
            Open::Part => DependencyOpen::Part,
            Open::Nobody => DependencyOpen::Nobody,
        },
        through,
        can_give: dependency.can_give,
    })
}

impl Engine {
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the last half of a plan is made from everything the first half found"
    )]
    async fn assemble(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        source: &Source,
        target: &Target,
        request: &ForkRequest,
        history: &orch_api_contract::forks::ForkHistory,
        options: AppliedChangeOptions,
        options_listed: &[binding::ArrangementOption],
        approval_mode: ApprovalMode,
        mut blockers: Blockers,
        reach: Reach,
        now: DateTime<Utc>,
    ) -> Result<Draft, PlanError> {
        let thread_id = source.thread.node_id;
        let replace = request.result == ForkResult::Replace;
        let person = principal.user();
        let source_files = crate::fork_source::plan(
            self,
            conn,
            principal,
            request,
            &source.thread,
            &source.environment,
            &target.directory,
            match &target.landing {
                Landing::Same => Some(&source.environment),
                Landing::Existing(row) => Some(row.as_ref()),
                Landing::New { .. } => None,
            },
        )
        .await?;
        for blocker in &source_files.blockers {
            blockers.add(&blocker.code, &blocker.message);
        }
        let environment_changes =
            matches!(target.landing, Landing::Existing(_) | Landing::New { .. });
        let differs = environment_changes || target.directory != source.thread.workspace_dir;

        // ---- the history
        let last_completed = orch_db::queries::thread_history_cuts::last_completed(
            conn,
            thread_id,
            source.thread.active_continuation_id,
        )
        .await?;
        let last_completed = last_completed.max(
            crate::calls::carried_failure(conn, thread_id, source.thread.active_continuation_id)
                .await?,
        );
        let range = if history.mode == HistoryMode::Nothing {
            None
        } else {
            turn_range(request, last_completed, history.mode)?
        };
        if history.mode != HistoryMode::Nothing
            && orch_db::queries::thread_history_cuts::unconfirmed_inputs(
                conn,
                thread_id,
                source.thread.active_continuation_id,
                Some(range.unwrap_or((1, 0))),
            )
            .await?
        {
            blockers.add("history_source_unconfirmed","This range contains words whose earlier native delivery is unconfirmed. They remain in their original queue; this fork cannot copy them as delivered history.");
        }
        // What the history holds is counted in SQL: the plan never loads it, whatever its length.
        let (low, high) = range.map_or((1, 0), |(from, to)| (i64::from(from), i64::from(to)));
        let prefix = if history.mode != HistoryMode::Nothing && low == 1 {
            if let Some(parent) = source.thread.side_of {
                let cut = orch_db::queries::thread_history_cuts::cut(conn, thread_id)
                    .await?
                    .filter(|cut| {
                        cut.thread_id == parent && cut.owner_thread_id == Some(thread_id)
                    });
                if cut.is_none() {
                    blockers.add(
                        "history_source_unconfirmed",
                        "The side's captured parent history is unavailable.",
                    );
                }
                cut
            } else {
                None
            }
        } else {
            None
        };
        let mut counts = if history.mode == HistoryMode::Nothing {
            crate::fork_history::Counts::default()
        } else {
            let counted = orch_db::queries::thread_history::range_counts(
                conn,
                thread_id,
                source.thread.active_continuation_id,
                low,
                high,
            )
            .await?;
            crate::fork_history::Counts {
                turns: counted.turns,
                items: counted.items,
                left_out_before: i32::try_from(low).ok().filter(|low| *low > 1),
                bytes: counted.bytes,
            }
        };
        if let Some(prefix) = &prefix {
            let inherited = orch_db::queries::thread_history_cuts::counts(conn, prefix.id).await?;
            counts.turns = counts.turns.saturating_add(inherited.turns);
            for (kind, count) in inherited.items {
                if kind != "reasoning" {
                    let held = counts.items.entry(kind).or_default();
                    *held = held.saturating_add(count);
                }
            }
            // The proof limit includes the whole retained prefix, including reasoning, just as the proof does.
            counts.bytes = counts.bytes.saturating_add(
                orch_db::queries::thread_history_cuts::payload_bytes(conn, prefix.id).await?,
            );
        }
        let turns = counts.turns;
        let source_machine = harnesses::machine_of(
            source.thread.arrangement.as_deref(),
            &source.provider,
            source.environment.machine_id,
        );
        let target_environment_machine = match &target.landing {
            Landing::Same => source.environment.machine_id,
            Landing::Existing(row) => row.machine_id,
            Landing::New { .. } => None,
        };
        let target_arrangement_word = target
            .arrangement
            .as_ref()
            .map(|chosen| chosen.kind.as_str());
        let target_machine = harnesses::machine_of(
            target_arrangement_word,
            &target.provider,
            target_environment_machine,
        );
        let same_harness_process = source.provider.id == target.provider.id
            && source.agent.id == target.agent.id
            && source_machine.is_some()
            && source_machine == target_machine;
        let source_reachable = match source_machine {
            Some(machine) => orch_db::queries::resources::machine(&mut *conn, machine)
                .await?
                .is_some_and(|row| row.online),
            None => false,
        };
        let method_wanted = HistoryRequest {
            mode: convert::<_, AgentMode>(&history.mode)?,
            is_switch: replace,
            same_harness_process,
            from_first_turn: range.is_none_or(|(from, _)| from == 1),
            same_kind: source.agent.harness_kind == target.agent.harness_kind,
            source_reachable,
        };
        // A native import fills the new agent's context with the turns: where they would pass the place's share
        // of the new model's window, the transcript goes as a file instead, with the recent turns that fit
        // carried beside it.
        let window = resources::model(&mut *conn, target.provider.id, &target.model)
            .await?
            .and_then(|model| crate::fork_history::model_window(&model.parameters));
        let budget = crate::fork_history::carry_budget(&mut *conn, target.topic, window).await?;
        let mut declined = Vec::new();
        if let Some(segment) = source.thread.current_segment_id
            && (orch_db::queries::agent_history::get(conn, segment)
                .await?
                .is_some_and(|history| history.removed_at.is_some())
                || Self::agent_history_handoff_pending(conn, &source.thread, segment).await?)
        {
            declined.extend([
                AgentMethod::SameSession,
                AgentMethod::NativeFork,
                AgentMethod::NativeTransplant,
            ]);
        }
        // A completed build check still applies after profile overrides and after an older report.
        let selected_descriptor = harnesses::descriptor_at(
            &mut *conn,
            &target.agent,
            target.provider.id,
            target_machine,
        )
        .await?
        .unwrap_or_else(|| target.descriptor.clone());
        let (limited, off) =
            crate::hosting::build_limits(&mut *conn, serde_json::to_value(selected_descriptor)?)
                .await?;
        let limited: AgentDescriptor = serde_json::from_value(limited)?;
        let native_import_unavailable =
            off.iter()
                .any(|feature| feature == "import_native")
                .then(|| orch_api_contract::admin::RunningHarnessBuild {
                    kind: limited.kind.clone(),
                    harness: Some(crate::programs::shipped_name(
                        &limited.kind,
                        serde_json::to_value(&limited).ok().as_ref(),
                    )),
                    version: limited.harness.version.clone(),
                    build: None,
                });
        let decide = |declined: &[AgentMethod]| {
            if turns == 0 && counts.items_total() == 0 {
                AgentMethod::None
            } else {
                choose_method(
                    &method_wanted,
                    &source.descriptor.capabilities,
                    &limited.capabilities,
                    declined,
                )
            }
        };
        let mut method: HistoryMethod = convert(&decide(&declined))?;
        // A native fork point is proved against the whole record of the session, which an adapter keeps to 32 MiB at most
        // (`thread.native_fork_check_bytes`): a history that takes more cannot be proved, and the plan says so rather than
        // promise what the act cannot do.
        let mut too_long_to_prove = false;
        if matches!(
            method,
            HistoryMethod::NativeFork | HistoryMethod::NativeTransplant
        ) {
            let limit = crate::fork_history::native_proof_limit(&mut *conn, thread_id).await?;
            if counts.bytes > limit
                || counts.items_total() > crate::fork_history::native_proof_items(limit)
            {
                declined.extend([AgentMethod::NativeFork, AgentMethod::NativeTransplant]);
                method = convert(&decide(&declined))?;
                too_long_to_prove = true;
            }
        }
        // Where a neutral import is what would be chosen, whether all of it can be told as items within the share of the
        // new model's window is found by reading its newest turns until the share is used: what that costs follows the
        // share, not the length of the history. Where it cannot, the transcript goes as a file with the recent turns beside
        // it. The other methods read nothing here.
        let mut neutral = None;
        if method == HistoryMethod::NeutralImport {
            let view = source.thread.active_continuation_id;
            let last =
                orch_db::queries::thread_history::last_turn(&mut *conn, thread_id, view).await?;
            let limit = budget.unwrap_or(crate::fork_history::UNKNOWN_WINDOW_NEUTRAL_BYTES);
            let mut read = crate::fork_history::newest(
                &mut *conn,
                crate::history_pages::Source::Live {
                    thread: thread_id,
                    view,
                },
                (low.max(1), high),
                last,
                limit,
            )
            .await?;
            if read.complete
                && let Some(prefix) = &prefix
            {
                // Side turn numbers follow the prefix in the captured archive. Account for any
                // extra digits before deciding how much of the window remains for the prefix.
                let mut extra_digits = std::collections::HashMap::new();
                for (id, ordinal) in &read.selected.turns {
                    let shifted = ordinal.checked_add(prefix.last_turn).ok_or_else(|| {
                        EngineError::unreadable("The captured turn order is too large.")
                    })?;
                    extra_digits.insert(
                        *id,
                        shifted
                            .to_string()
                            .len()
                            .saturating_sub(ordinal.to_string().len()),
                    );
                }
                for item in &read.selected.items {
                    if let Some(extra) = item.turn_id.and_then(|id| extra_digits.get(&id)) {
                        read.bytes = read.bytes.saturating_add(*extra);
                    }
                }
                if read.bytes > limit {
                    read.complete = false;
                } else {
                    let inherited = crate::fork_history::newest(
                        conn,
                        crate::history_pages::Source::Cut(prefix),
                        (1, i64::from(prefix.last_turn)),
                        i64::from(prefix.last_turn),
                        limit - read.bytes,
                    )
                    .await?;
                    read.complete = inherited.complete;
                    read.bytes = read.bytes.saturating_add(inherited.bytes);
                }
            }
            if read.complete {
                neutral = Some(read);
            } else {
                declined.push(AgentMethod::NeutralImport);
                method = convert(&decide(&declined))?;
            }
        }
        let told = neutral
            .as_ref()
            .filter(|_| method == HistoryMethod::NeutralImport);
        let mut report = crate::fork_history::report_counts(
            method,
            &counts,
            Some(&told.map_or_else(
                || crate::fork_history::carried_words(budget, window),
                |neutral| crate::fork_history::neutral_words(neutral.bytes, window),
            )),
        );
        if too_long_to_prove {
            report.warnings.push(
                "this conversation is too long for the program to fork it at this turn itself, so the newest turns are told to the new agent instead"
                    .to_owned(),
            );
        }
        let archive_bytes = told.map(|neutral| i64::try_from(neutral.bytes).unwrap_or(i64::MAX));

        // ---- the files and the notes
        let carry = carry_of(request);
        let source_root = crate::sessions::workspace_root(
            &source.environment.workspace_root,
            &source.environment.facts,
            source.environment.generation,
        );
        let target_root = match &target.landing {
            Landing::Existing(row) => {
                crate::sessions::workspace_root(&row.workspace_root, &row.facts, row.generation)
            }
            _ => source_root,
        };
        let live_tree =
            source.environment.facts.get("index").and_then(Json::as_str) == Some("live");
        let selection_required = differs
            && live_tree
            && !matches!(carry.selection.as_ref(),
            Some(FileSelection::Selection { include, .. }) if !include.is_empty() && include.iter().all(|path| !path.is_empty()))
            && !matches!(carry.selection, Some(FileSelection::Nothing));
        if selection_required {
            blockers.add(
                "selection_required",
                "this workspace is listed live; choose explicit paths before copying it",
            );
        }
        let source_mounts = crate::mounts::views(conn, source.environment.id, now).await?;
        let mut target_copy_driver = None;
        // A new environment's mounts are resolved as it would be made: with the locations its machine has, and
        // again with what the plan copies there and detaches.
        let mut prospect = None;
        let target_plain = match &target.landing {
            Landing::Same => source_mounts.clone(),
            Landing::Existing(row) => crate::mounts::views(conn, row.id, now).await?,
            Landing::New { host, .. } => {
                if let Some(ForkEnvironment::New { new }) = &request.environment {
                    let home = new.home_node_id.unwrap_or(target.topic);
                    let own = new.config.clone().unwrap_or_else(|| serde_json::json!({}));
                    let template_id = match new.template_id {
                        Some(id) => Some(id),
                        None => {
                            setting(conn, SettingsPlace::Node(home), keys::ENV_TEMPLATE_ID).await?
                        }
                    };
                    let template = match template_id {
                        Some(id) => {
                            orch_db::queries::resources::hosting::template(conn, id).await?
                        }
                        None => None,
                    };
                    target_copy_driver = template.as_ref().map(|template| template.driver.clone());
                    let storage =
                        crate::places::resolved_for_new(conn, home, &own, keys::ENV_STORAGE)
                            .await?;
                    let made = Prospect {
                        home,
                        own,
                        machine: host.as_ref().map(|host| host.host.machine_id),
                        driver: template.as_ref().map(|template| template.driver.clone()),
                        storage: storage
                            .as_ref()
                            .and_then(|storage| storage.get("workspace"))
                            .and_then(Json::as_str)
                            .map(str::to_owned),
                        template_spec: template.as_ref().map(|template| template.spec.clone()),
                    };
                    let views =
                        crate::mounts::resolve_for(conn, &made.target(person, &[], &[]), now)
                            .await?
                            .into_iter()
                            .map(|mount| mount.view)
                            .collect();
                    prospect = Some(made);
                    views
                } else {
                    Vec::new()
                }
            }
        };
        let source_machine = crate::mounts::location_machine(conn, source.environment.id).await?;
        let target_machine = match &target.landing {
            Landing::Same => source_machine,
            Landing::Existing(row) => crate::mounts::location_machine(conn, row.id).await?,
            Landing::New { .. } => prospect.as_ref().and_then(|made| made.machine),
        };
        let decided = crate::mount_copies::decide(
            self,
            conn,
            &crate::mount_copies::Ask {
                choices: request.mounts.as_deref().unwrap_or_default(),
                check_fingerprints: request.check_fingerprints.unwrap_or(false).then(|| {
                    request
                        .fingerprint_request_id
                        .unwrap_or_else(orch_types::new_id)
                }),
                // Only a new environment is made with the mounts the plan chooses; one used as it stands keeps its own.
                offer: if matches!(target.landing, Landing::New { .. }) {
                    crate::mount_copies::Offer::Everything
                } else {
                    crate::mount_copies::Offer::Unchanged
                },
                source: &source_mounts,
                target: &target_plain,
                source_machine,
                target_machine,
                principal,
                at: now,
            },
        )
        .await?;
        for found in &decided.blockers {
            blockers.add(&found.code, found.message.clone());
        }
        let target_mounts = match &prospect {
            Some(made)
                if !decided.decisions.detached.is_empty()
                    || !decided.decisions.copies.is_empty() =>
            {
                let planned = decided.decisions.planned(made.machine, person, now);
                crate::mounts::resolve_for(
                    conn,
                    &made.target(person, &planned, &decided.decisions.detached),
                    now,
                )
                .await?
                .into_iter()
                .map(|mount| mount.view)
                .collect()
            }
            _ => target_plain,
        };
        let mount_plan = crate::mounts::plan(&source_mounts, &target_mounts, &decided.fated);
        let source_servers =
            crate::mcp::resolve(conn, source.environment.id, Time::new(now)).await?;
        let target_servers = match (&target.landing, &prospect) {
            (Landing::Same, _) => source_servers.clone(),
            (Landing::Existing(row), _) => {
                crate::mcp::resolve(conn, row.id, Time::new(now)).await?
            }
            (Landing::New { .. }, Some(made)) => {
                crate::mcp::resolve_elsewhere(
                    conn,
                    &crate::mcp::Elsewhere::New {
                        home: made.home,
                        owner: person,
                        machine: made.machine,
                        template: made.template_spec.as_ref(),
                        own: &made.own,
                    },
                    Time::new(now),
                )
                .await?
            }
            (Landing::New { .. }, None) => Vec::new(),
        };
        let mcp_plan = crate::mcp::plan(&source_servers, &target_servers);
        let mut private_views = if environment_changes {
            let carry_private = match &request.environment {
                Some(ForkEnvironment::New { new }) => new.private_views.unwrap_or(true),
                _ => false,
            };
            crate::mounts::private_plan(
                &source_mounts,
                &target_mounts,
                carry_private,
                &decided.decisions.detached,
            )
        } else {
            Vec::new()
        };
        let home_plan = match crate::volume_carry::measure(
            self,
            conn,
            source.environment.id,
            carry.home,
            principal.user(),
            &mut private_views,
        )
        .await
        {
            Ok(home) => home,
            Err(_) => {
                if carry.home || private_views.iter().any(|view| view.carried) {
                    blockers.add("volume_measurement_unavailable", "the old host could not measure the whole home/private volumes; no carry can be confirmed");
                }
                carry.home.then_some(FilesHome {
                    measurement: None,
                    measured: false,
                    file_count: 0,
                    total_bytes: 0,
                })
            }
        };
        if (carry.home || private_views.iter().any(|view| view.carried))
            && let (
                Landing::New {
                    host: Some(host), ..
                },
                Some(driver),
            ) = (&target.landing, target_copy_driver.as_deref())
            && let Err(error) =
                crate::volume_carry::check_target(self, host.host.machine_id, driver).await
        {
            blockers.add("volume_copy_unavailable", error.to_string());
        }
        let (volume_files, volume_bytes) =
            crate::volume_carry::totals(home_plan.as_ref(), &private_views);
        let nothing_copied = matches!(carry.selection, Some(FileSelection::Nothing));
        let mut copy_worktree = false;
        let (mut files, memory, listing_bytes) = if differs && !selection_required {
            let mut listing = if nothing_copied {
                Listing {
                    as_of: now,
                    files: 0,
                    bytes: 0,
                    without_content: 0,
                    bytes_without_content: 0,
                    sample: Vec::new(),
                    notes_files: 0,
                    notes_bytes: 0,
                }
            } else if live_tree && matches!(carry.selection, Some(FileSelection::Selection { .. }))
            {
                if source.environment.status != "running" {
                    blockers.add("files_unavailable", "connect this live workspace to measure the selected paths before confirming");
                    self.list_directory(
                        conn,
                        source.environment.id,
                        &source.thread.workspace_dir,
                        carry.selection.as_ref(),
                        now,
                    )
                    .await?
                } else {
                    match self
                        .list_selected_live(
                            conn,
                            &source.environment,
                            &source.thread.workspace_dir,
                            carry.selection.as_ref().ok_or_else(|| {
                                EngineError::unreadable("the selection is missing")
                            })?,
                        )
                        .await
                    {
                        Ok(listing) => listing,
                        Err(_) => {
                            blockers.add("files_unavailable", "the source could not measure the selected paths; no copy has started");
                            self.list_directory(
                                conn,
                                source.environment.id,
                                &source.thread.workspace_dir,
                                carry.selection.as_ref(),
                                now,
                            )
                            .await?
                        }
                    }
                }
            } else {
                self.list_directory(
                    &mut *conn,
                    source.environment.id,
                    &source.thread.workspace_dir,
                    carry.selection.as_ref(),
                    now,
                )
                .await?
            };
            let online = source.environment.status == "running";
            if !nothing_copied && request.worktree.is_none() && request.files_at.is_none() {
                let selection = crate::jobs::fork::snapshot_selection(
                    &source.thread.workspace_dir,
                    carry.selection.as_ref(),
                    None,
                );
                if online {
                    match crate::fork_source::independent_plan(self, &source.environment, &source.thread.workspace_dir, selection).await {
                        Ok(extra) => { copy_worktree = extra.independent;
                            listing.files = listing.files.saturating_add(extra.additional_files);
                            listing.bytes = listing.bytes.saturating_add(extra.additional_bytes); },
                        Err(_) => blockers.add("worktree_copy_unavailable", "the source must provide all repository objects and refs before this directory can be copied"),
                    }
                } else if source
                    .thread
                    .source
                    .as_ref()
                    .and_then(|source| source.pointer("/info/kind"))
                    .and_then(Json::as_str)
                    == Some("worktree")
                {
                    blockers.add(
                        "worktree_copy_unavailable",
                        "connect the source worktree before copying its independent repository",
                    );
                }
            }
            let root_of =
                |root: &str, directory: &str| crate::sessions::absolute_directory(root, directory);
            let files =
                (!nothing_copied || home_plan.is_some() || !private_views.is_empty()).then(|| {
                    FilesPlan {
                        kept_image_bytes: None,
                        source: if online {
                            FilesSource::Live
                        } else {
                            FilesSource::Cached
                        },
                        source_as_of: Time::new(listing.as_of),
                        file_count: if nothing_copied { 0 } else { listing.files },
                        total_bytes: if nothing_copied { 0 } else { listing.bytes },
                        bytes_to_upload: (if online && !nothing_copied {
                            listing.bytes
                        } else {
                            0
                        })
                        .saturating_add(volume_bytes),
                        bytes_to_download: (if nothing_copied { 0 } else { listing.bytes })
                            .saturating_add(volume_bytes),
                        missing: FilesMissing {
                            count: listing.without_content,
                            bytes: Some(listing.bytes_without_content),
                            sample: listing.sample.clone(),
                        },
                        path_map: PathMap {
                            from: root_of(source_root, &source.thread.workspace_dir),
                            to: root_of(target_root, &target.directory),
                        },
                        home: home_plan.clone(),
                        file_states: None,
                        private_views: private_views.clone(),
                    }
                });
            let memory = (listing.notes_files > 0 && crate::memory::kept(source.settings.memory))
                .then(|| ForkPlanMemory {
                    carried: crate::memory::carried(
                        request.memory,
                        replace,
                        history.mode != HistoryMode::Nothing,
                    ),
                    files: listing.notes_files,
                    bytes: listing.notes_bytes,
                });
            let bytes = files.as_ref().map(|plan| {
                (
                    plan.bytes_to_upload,
                    plan.bytes_to_download,
                    plan.file_count.saturating_add(volume_files),
                )
            });
            (files, memory, bytes)
        } else {
            (None, None, None)
        };

        // ---- who, and who can go on
        let first_owner = user_ref(&mut *conn, source.thread.owner_id).await?;
        let owner = if replace {
            ForkPlanOwner {
                before: Some(first_owner.clone()),
                after: first_owner,
            }
        } else {
            ForkPlanOwner {
                before: None,
                after: user_ref(&mut *conn, person).await?,
            }
        };
        let mut depends = Vec::new();
        if replace && target.provider.id != source.provider.id {
            let policies = crate::places::policies_at(&mut *conn, source.thread.topic_id).await?;
            for found in orch_authz::depends::of_thread(
                &mut *conn,
                principal,
                thread_id,
                target.provider.id,
                &target.model,
                target.effort.as_deref(),
                &policies,
            )
            .await?
            {
                depends.push(dependency_view(&mut *conn, &found).await?);
            }
        }

        // ---- what is left behind
        let leaves = if replace {
            let environment = if environment_changes {
                let others = orch_db::queries::threads::tools::threads_in_environment(
                    &mut *conn,
                    source.environment.id,
                )
                .await?
                .into_iter()
                .filter(|other| *other != thread_id)
                .count();
                let may_remove = self
                    .inner
                    .access
                    .environment_manage(&mut *conn, person, source.environment.id)
                    .await
                    .map_err(EngineError::from)?
                    .is_allowed();
                let action = if options.keep_previous {
                    LeftAction::Keep
                } else if others > 0 {
                    LeftAction::InUse
                } else if !orch_db::queries::environment_facts::has_unit(
                    conn,
                    source.environment.id,
                )
                .await?
                    || !may_remove
                {
                    LeftAction::NotRemovable
                } else {
                    LeftAction::Remove
                };
                Some(LeftEnvironment {
                    environment: environment_ref(&mut *conn, &source.environment, Time::new(now))
                        .await?,
                    action,
                    processes: 0,
                })
            } else {
                None
            };
            Some(ForkPlanLeaves {
                session: true,
                environment,
            })
        } else {
            None
        };

        // ---- how long
        let rate_of = |row: Option<orch_db::queries::resources::MachineRow>, up: bool| {
            Rate::of(row.and_then(|row| {
                if up {
                    row.rate_up_bps
                } else {
                    row.rate_down_bps
                }
            }))
        };
        let source_row = match source.environment.machine_id {
            Some(machine) => orch_db::queries::resources::machine(&mut *conn, machine).await?,
            None => None,
        };
        let target_row = match target_environment_machine {
            Some(machine) => orch_db::queries::resources::machine(&mut *conn, machine).await?,
            None => None,
        };
        let rates = Rates {
            up: rate_of(source_row, true),
            down: rate_of(target_row, false),
        };
        let making = match &target.landing {
            Landing::New { .. } => Making::Environment,
            Landing::Existing(row)
                if row.status != "running"
                    && orch_db::queries::environment_facts::has_unit(conn, row.id).await? =>
            {
                Making::StartRuntime
            }
            _ if target.provider.id != source.provider.id => Making::StartRuntime,
            _ => Making::Nothing,
        };
        let (up, down, count) = listing_bytes.unwrap_or((0, 0, 0));
        let estimate: Estimate = estimates::estimate(
            &Work {
                bytes_up: up,
                bytes_down: down,
                files: count,
                archive_bytes,
                making,
            },
            &rates,
        );

        // ---- the plan
        let option = target
            .arrangement
            .as_ref()
            .and_then(|chosen| {
                options_listed
                    .iter()
                    .find(|option| option.kind == chosen.kind)
            })
            .or_else(|| options_listed.first())
            .ok_or_else(|| EngineError::unreadable("the profile declares no arrangement"))?;
        let arrangement_kind = target
            .arrangement
            .as_ref()
            .map_or(option.kind, |chosen| chosen.kind);
        let to_environment = match &target.landing {
            Landing::Same => ForkPlanTarget::Existing(
                environment_ref(&mut *conn, &source.environment, Time::new(now)).await?,
            ),
            Landing::Existing(row) => {
                ForkPlanTarget::Existing(environment_ref(&mut *conn, row, Time::new(now)).await?)
            }
            Landing::New {
                name,
                host: Some(host),
                ..
            } => ForkPlanTarget::Made {
                new: PlannedEnvironment {
                    name: name.clone(),
                    place: orch_api_contract::forks::Place::Host(host.place.clone()),
                },
            },
            Landing::New { host: None, .. } => ForkPlanTarget::Existing(
                environment_ref(&mut *conn, &source.environment, Time::new(now)).await?,
            ),
        };
        let profile_environment = match &target.landing {
            Landing::Existing(row) => Some(row.id),
            Landing::Same => Some(source.environment.id),
            _ => None,
        };
        let profile = match profile_environment {
            Some(id) => match orch_db::queries::environment_facts::access_profiles(conn, &[id])
                .await?
                .first()
                .map(|(_, profile)| *profile)
            {
                Some(orch_access_contract::permissions::Placement::Connected) => {
                    orch_api_contract::access::PageProfile::Connected
                }
                _ => orch_api_contract::access::PageProfile::Managed,
            },
            None => orch_api_contract::access::PageProfile::Managed,
        };
        let running = threads::active_turn(&mut *conn, thread_id).await?.is_some();
        if source_files.worktree.is_some() {
            files = None;
        }
        if let Some(saved) = &source_files.files_at
            && request.worktree.is_none()
        {
            files = Some(FilesPlan {
                kept_image_bytes: None,
                source: FilesSource::Cached,
                source_as_of: saved.created_at,
                file_count: saved.file_count,
                total_bytes: saved.total_bytes,
                bytes_to_upload: saved.total_bytes,
                bytes_to_download: saved.total_bytes,
                missing: FilesMissing {
                    count: 0,
                    bytes: Some(0),
                    sample: Vec::new(),
                },
                path_map: PathMap {
                    from: crate::sessions::absolute_directory(
                        &source.environment.workspace_root,
                        saved.directory.as_deref().unwrap_or_default(),
                    ),
                    to: crate::sessions::absolute_directory(target_root, &target.directory),
                },
                home: home_plan.clone(),
                file_states: None,
                private_views: private_views.clone(),
            });
        }
        let plan = ForkPlan {
            preflight_id: Id::nil(),
            expires_at: Time::new(now + Duration::minutes(PREFLIGHT_VALID_MINUTES)),
            source: thread_ref(&source.thread),
            result: request.result,
            from: ForkPlanFrom {
                agent: agent_ref(&mut *conn, &source.agent).await?,
                provider: provider_ref(&mut *conn, &source.provider).await?,
                environment: Some(
                    environment_ref(&mut *conn, &source.environment, Time::new(now)).await?,
                ),
            },
            to: ForkPlanTo {
                agent: agent_ref(&mut *conn, &target.agent).await?,
                provider: provider_ref(&mut *conn, &target.provider).await?,
                model: target.model.clone(),
                effort: target.effort.clone(),
                approval_mode: Some(convert(&approval_mode)?),
                arrangement: convert(&arrangement_kind)?,
                isolation: isolation_view(
                    option,
                    &target.provider,
                    &target.agent,
                    &target.descriptor,
                )?,
                predicted: matches!(to_environment, ForkPlanTarget::Made { .. }),
                environment: to_environment,
                directory: target.directory.clone(),
            },
            history: ForkPlanHistory {
                native_import_unavailable,
                mode: history.mode,
                turns,
                method,
                source_available: source_reachable && source.descriptor.capabilities.export_native,
                report,
                archive_bytes,
                summary: if history.mode == orch_api_contract::forks::HistoryMode::Summary {
                    Some(
                        crate::summaries::handoff_estimate(
                            &mut *conn,
                            source.thread.node_id,
                            turns,
                        )
                        .await?,
                    )
                } else {
                    None
                },
            },
            files,
            worktree: source_files.worktree,
            files_at: source_files.files_at,
            memory,
            mounts: mount_plan,
            mcp: mcp_plan,
            options,
            access: ForkPlanAccess {
                summary: orch_api_contract::access::AccessSummary {
                    view: Vec::new(),
                    interact: Vec::new(),
                    manage: Vec::new(),
                    custom: false,
                    from: None,
                },
                profile,
                differs: Vec::new(),
            },
            owner,
            depends,
            locks: if replace {
                vec![thread_ref(&source.thread)]
            } else {
                Vec::new()
            },
            leaves,
            estimate,
            turn_running: running,
            blockers: std::mem::take(&mut blockers.list),
        };
        // The history the plan shows is the one carried out, even when the place's default changes before confirming.
        Ok(Draft {
            request: ForkRequest {
                history: Some(history.clone()),
                ..request.clone()
            },
            plan,
            lacks: blockers.lacks,
            copy_worktree,
            source_scope: SourceScope::of(&source.thread, reach),
            mount_decisions: decided.decisions,
        })
    }
}

// ---------------------------------------------------------------- keeping it, and finding it again

impl Engine {
    /// Keeps a plan for the minutes it is good for, under the identifier a request to carry it out names,
    /// and returns it with that identifier. The layer that names objects has filled in what it says by now.
    pub async fn keep_fork_plan(
        &self,
        caller: &Caller,
        mut draft: Draft,
    ) -> Result<ForkPlan, PlanError> {
        let id = new_id();
        draft.plan.preflight_id = id;
        let stored = StoredPlan {
            request: draft.request.clone(),
            plan: draft.plan.clone(),
            source: draft.plan.source.id,
            source_scope: Some(draft.source_scope),
        };
        let mut conn = self.inner.db.acquire().await?;
        let now = self.inner.clock.now();
        fork_rows::save_preflight(
            &mut conn,
            &fork_rows::PreflightRow {
                id,
                kind: fork_rows::PLAN_KIND.to_owned(),
                node_id: Some(stored.source),
                environment_id: None,
                created_by: caller.user(),
                plan: serde_json::to_value(&stored)?,
                expires_at: draft.plan.expires_at.get(),
                created_at: now,
            },
        )
        .await?;
        Ok(draft.plan)
    }

    /// The plan the caller asked for earlier and names now: theirs, good yet, and about this thread.
    pub(crate) async fn load_fork_plan(
        &self,
        conn: &mut Conn,
        caller: &Caller,
        preflight: Id,
        thread: Option<Id>,
    ) -> Result<(fork_rows::PreflightRow, StoredPlan), PlanError> {
        let row = fork_rows::preflight(&mut *conn, preflight)
            .await?
            .filter(|row| row.kind == fork_rows::PLAN_KIND)
            .filter(|row| row.created_by == caller.user())
            .filter(|row| thread.is_none() || row.node_id == thread)
            .ok_or(PlanError::NoSuchPlan)?;
        if row.expires_at <= self.inner.clock.now() {
            return Err(PlanError::Expired);
        }
        let stored: StoredPlan = serde_json::from_value(row.plan.clone())?;
        Ok((row, stored))
    }
}

/// Confirmation/job readers already own their history identity; never resolve
/// another active continuation while a confirmed operation is in flight.
pub(crate) async fn history_range_view(
    conn: &mut Conn,
    thread: Id,
    view: Id,
    request: &ForkRequest,
) -> Result<Option<(i32, i32)>, PlanError> {
    let mode = request
        .history
        .as_ref()
        .map_or(HistoryMode::All, |history| history.mode);
    if mode == HistoryMode::Nothing {
        return Ok(None);
    }
    let last = orch_db::queries::thread_history_cuts::last_completed(conn, thread, view)
        .await?
        .max(crate::calls::carried_failure(conn, thread, view).await?);
    turn_range(request, last, mode)
}

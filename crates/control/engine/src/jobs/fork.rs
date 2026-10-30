//! The one job behind fork and switch: lock, provision, copy, history, verify, commit, cleanup; everything undone before the commit; history up to any turn, and handed to another agent in the conversation's written form; the agent's notes taken along as the plan says; a switch that keeps owners and page and asks only for what it changes, and a fork that is its maker's with the page of its source.
//!
//! **Confirming** a plan makes the job and takes the lock in one transaction ([`Engine::confirm_fork`]):
//! the plan is worked out again from the request that was kept, so that what is carried out is what is
//! true now; a plan with blockers is refused; the thread a switch changes is held from that moment, since
//! an open job of this kind is what holds it ([`crate::locks`]).
//!
//! **The phases** are `lock`, `provision`, `copy`, `history`, `verify`, `commit` and `cleanup`, each restartable, each leaving in the job's plan what it made so
//! that the next run, or another process, goes on from there and the undo knows what to take back:
//!
//! - `lock`: for a switch, the approvals that wait are refused and the running turn is waited for (up to
//!   the minutes the options give) or interrupted at once; for a fork, the new thread is made, held like a
//!   switched one until the job is done.
//! - `provision`: a new environment is made at the planned place, or one that exists is started; the
//!   target provider's runtime and harness are started.
//! - `copy`: the files go from the source to the target, by a snapshot and a write that are asked again for
//!   what changed meanwhile; what is already there with the right hash is skipped.
//! - `history`: by the method the plan chose, the session of the target is opened with the history
//!   (resumed, forked by the harness, imported from the harness's own archive or from the neutral one, or
//!   begun fresh beside a transcript); the harness's own report of what it kept replaces the plan's.
//! - `verify`: the target is there and the session holds the history, within the time the options give.
//! - `commit`: one transaction makes the new side the real one.
//! - `cleanup`: the old session is closed and what was left behind is scheduled for removal; nothing here
//!   fails the job, since after the commit nothing is undone.
//!
//! **Undoing.** A job that fails or is cancelled before its commit closes the session it opened, removes the
//! environment it made, deletes the thread it made, and lets the thread it held go, with its queue.

use crate::commands::Outgoing;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::fork_history::{self, Selected};
use crate::fork_plan::{Draft, PlanError};
use crate::harnesses::{self, HarnessKey};
use crate::ids;
use crate::jobs::runner::{
    Done, Failure, INTERNAL, Job, Phase, Phased, Stop, Undone, initial_progress,
};
use crate::unit::Unit;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_api_contract::forks::{
    ActiveTurn, AppliedChangeOptions, FileSelection, ForkCreateBody, ForkEnvironment, ForkPlan,
    ForkPlanTarget, ForkRequest, ForkResult, HistoryMethod, HistoryReport,
};
use orch_authz::Caller;
use orch_db::queries::environments::hosting as hosting_rows;
use orch_db::queries::environments::thread_engine as environment_rows;
use orch_db::queries::jobs::{JobRow, forks as fork_rows};
use orch_db::queries::resources::thread_engine as resource_rows;
use orch_db::queries::threads::{self, Binding, NewSegment, StateChange, forking, thread_state};
use orch_link_contract::commands::{
    FsMaterialize, FsRemove, FsSnapshot, FsSnapshotResult, MaterializePurpose, Selection,
    SerialKey, SessionExport, SessionExportResult, SessionMode, SessionOpen, SessionOpenArchive,
    SessionOpenHistory, SessionOpenResult,
};
use orch_types::{Id, Json, Time, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

mod recovery;
mod source;

/// The word `thread_fork` jobs have for their kind.
pub const KIND: &str = fork_rows::THREAD_FORK;

/// The first phase: the thread is held, or the new thread is made.
const LOCK: Phase = Phase {
    name: "lock",
    weight: 5,
};
/// The second: the target is made ready.
const PROVISION: Phase = Phase {
    name: "provision",
    weight: 15,
};
/// The third, where the time goes: the files.
const COPY: Phase = Phase {
    name: "copy",
    weight: 50,
};
/// The fourth: the history.
const HISTORY: Phase = Phase {
    name: "history",
    weight: 10,
};
/// The fifth: the checks.
const VERIFY: Phase = Phase {
    name: "verify",
    weight: 10,
};
/// The sixth: the one transaction.
const COMMIT: Phase = Phase {
    name: "commit",
    weight: 5,
};
/// The last: what was left behind.
const CLEANUP: Phase = Phase {
    name: "cleanup",
    weight: 5,
};

/// What the name of a fork is followed by, when its maker names none.
const FORK_SUFFIX: &str = " (fork)";

/// An inherited lock prevents the confirmed target approval choice.
const APPROVAL_MODE_LOCKED: &str = "The place locks an approval mode that the target program does not offer. Ask a manager to change the lock or choose another program.";

/// A value as JSON, or the failure of a job that has made something it cannot write down.
fn to_json<T: Serialize>(value: &T) -> Result<Json, Stop> {
    serde_json::to_value(value).map_err(|error| failed(INTERNAL, error.to_string()))
}

/// How often a phase that waits for something looks at it again.
const LOOK_EVERY: Duration = Duration::from_millis(250);

/// How long a phase waits for an environment to come up: the longest a unit may take to be made.
const ENVIRONMENT_WAIT: Duration = Duration::from_secs(15 * 60);

/// A chosen no-op native method does not change what history was requested.
fn wants_history(request: &ForkRequest) -> bool {
    !request
        .history
        .as_ref()
        .is_some_and(|history| history.mode == orch_api_contract::forks::HistoryMode::Nothing)
}

/// The phases a plan needs, in order: a phase with nothing to do is left out, and the weights of the rest
/// are scaled to one hundred by the runner.
#[must_use]
pub fn phases_of(plan: &ForkPlan) -> Vec<Phase> {
    let replace = plan.result == ForkResult::Replace;
    let from_environment = plan
        .from
        .environment
        .as_ref()
        .map(|environment| environment.id);
    let moves_environment = match &plan.to.environment {
        ForkPlanTarget::Made { .. } => true,
        ForkPlanTarget::Existing(environment) => Some(environment.id) != from_environment,
    };
    let moves_harness = plan.from.provider.id != plan.to.provider.id
        || plan.from.agent.id != plan.to.agent.id
        || moves_environment;
    let mut phases = vec![LOCK];
    if moves_harness {
        phases.push(PROVISION);
    }
    if plan.files.is_some() || plan.worktree.is_some() || plan.files_at.is_some() {
        phases.push(COPY);
    }
    if plan.history.method != HistoryMethod::Nothing {
        phases.push(HISTORY);
    }
    phases.push(VERIFY);
    phases.push(COMMIT);
    if replace {
        phases.push(CLEANUP);
    }
    phases
}

// ---------------------------------------------------------------- what the job is asked

/// What a job is asked to do, as it is kept in the job's plan; the members its phases add come beside.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    /// The plan that was confirmed.
    pub preflight_id: Id,
    /// A switch or a fork. Held by the lock query: `replace` holds the thread named by `source_thread`.
    pub result: ForkResult,
    /// What was asked.
    pub request: ForkRequest,
    /// The plan as it was worked out when it was confirmed.
    pub forkplan: ForkPlan,
    /// The options that apply.
    pub options: AppliedChangeOptions,
    /// The thread it is asked of.
    pub source_thread: Id,
    /// The thread a fork is making; held like a switched one until the job is done.
    pub new_thread: Option<Id>,
    /// The segment the result begins with.
    pub new_segment: Id,
    /// Who confirmed it.
    pub by: Id,
    /// The environment the source is in.
    pub from_environment: Id,
    /// The environment the result lands in, when it exists already.
    pub to_environment: Option<Id>,
    /// The segment the source is in now.
    pub source_segment: Option<Id>,
    /// The batch it belongs to.
    pub batch_id: Option<Id>,
}

/// What the phases of a job remember in its plan, member by member.
mod made {
    /// The environment this job made.
    pub const ENVIRONMENT: &str = "new_environment";
    /// When the lock phase began to wait for the running turn.
    pub const WAITING_SINCE: &str = "waiting_since";
    /// The session this job opened: where, which, and what the harness said of the history.
    pub const SESSION: &str = "opened";
    /// The exact completed-turn range handed to the harness; a fork's source may go on meanwhile.
    pub const HISTORY_RANGE: &str = "carried_history_range";
    /// The files this job wrote: the directory on the target, and whether it was made by the job.
    pub const COPIED: &str = "copied";
    /// The thread's harness machine before the switch, to close its session afterwards.
    pub const OLD_MACHINE: &str = "old_machine";
}

/// A session the job opened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Opened {
    /// The machine of the harness.
    pub machine: Id,
    /// The thread the session is for.
    pub thread: Id,
    /// The harness's name for it.
    pub native_session_ref: String,
    /// How it was opened.
    pub mode: SessionMode,
    /// The method that was really used.
    pub method: HistoryMethod,
    /// What the harness said it kept, converted and dropped; none for a session that is not an import.
    pub report: Option<HistoryReport>,
    /// The prompt records committed before this opening was sent.
    #[serde(default)]
    pub model_inputs: Vec<Id>,
    /// Instruction sources for the segment once the job commits.
    #[serde(default)]
    pub instruction_sources: Json,
    /// The native reviewer's session fact, where one was requested.
    #[serde(default)]
    pub approval_reviewer: Option<orch_api_contract::settings::ApprovalReviewer>,
}

/// Where the files of the job were written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Copied {
    /// The environment.
    pub environment: Id,
    /// The directory, absolute.
    pub directory: String,
    /// Whether nothing was there before: the directory is the job's to remove.
    pub made_by_job: bool,
}

fn member<T: serde::de::DeserializeOwned>(plan: &Json, key: &str) -> Option<T> {
    plan.get(key)
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

fn failed(code: &str, message: impl Into<String>) -> Stop {
    Stop::Failed(Failure::new(code, message))
}

// ---------------------------------------------------------------- confirming

impl Engine {
    /// Confirms a plan of a fork or a switch: the job is made and, for a switch, the thread is held from
    /// this transaction on. The plan is worked out again first; one with blockers is refused.
    pub async fn confirm_fork(
        &self,
        caller: &Caller,
        thread: Id,
        body: &ForkCreateBody,
    ) -> Result<JobRow, PlanError> {
        let tx = self.inner.db.begin().await?;
        self.confirm_fork_in(tx, caller, thread, body).await
    }

    /// Confirms as [`Self::confirm_fork`] does, in a transaction whose caller was asked again once the request had
    /// come (the API's action guard): `principal` was loaded in `tx` by that check, and its caller is the one the
    /// plan is worked out again for and the job is made by.
    pub async fn confirm_fork_guarded(
        &self,
        tx: orch_db::Tx,
        principal: orch_authz::Principal,
        thread: Id,
        body: &ForkCreateBody,
    ) -> Result<JobRow, PlanError> {
        let caller = principal.caller().clone();
        self.confirm_fork_in(tx, &caller, thread, body).await
    }

    async fn confirm_fork_in(
        &self,
        mut tx: orch_db::Tx,
        caller: &Caller,
        thread: Id,
        body: &ForkCreateBody,
    ) -> Result<JobRow, PlanError> {
        let (_, stored) = self
            .load_fork_plan(&mut tx, caller, body.preflight_id, Some(thread))
            .await?;
        // The thread is locked first: two confirmations of one thread are made to take turns.
        let locked = threads::lock(&mut tx, thread)
            .await?
            .filter(|row| row.deleted_at.is_none())
            .ok_or(PlanError::NotFound)?;
        if !stored
            .source_scope
            .as_ref()
            .is_some_and(|scope| scope.matches(&locked))
        {
            return Err(PlanError::Expired);
        }
        if let Some(job) = crate::locks::held_by(&mut tx, thread).await? {
            return Err(PlanError::Held(job));
        }
        let mut request = stored.request.clone();
        // A fingerprint is the observation shown at preflight, never a condition of moving or starting work.
        request.check_fingerprints = None;
        request.fingerprint_request_id = None;
        if let Some(given) = &body.options {
            let mut options = request.options.take().unwrap_or_default();
            options.active_turn = given.active_turn.or(options.active_turn);
            options.wait_minutes = given.wait_minutes.or(options.wait_minutes);
            options.verify_timeout_secs = given.verify_timeout_secs.or(options.verify_timeout_secs);
            options.keep_previous = given.keep_previous.or(options.keep_previous);
            options.stop_processes = given.stop_processes.or(options.stop_processes);
            request.options = Some(options);
        }
        let draft = self
            .draft_in(
                &mut tx,
                caller,
                thread,
                &request,
                crate::fork_plan::Reach::Seen,
                None,
            )
            .await?;
        if !draft.is_clear() {
            return Err(PlanError::Blocked(Box::new(draft)));
        }
        let job = self
            .start_fork_job(&mut tx, caller, &locked, draft, None, body.preflight_id)
            .await?;
        fork_rows::forget_preflight(&mut tx, body.preflight_id).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.kick();
        self.wake(thread);
        Ok(job)
    }

    /// Makes the job of a plan that has no blockers in the transaction of the caller, and takes the lock:
    /// a switch of a thread that has no turn running leaves it blocked from now on.
    pub(crate) async fn start_fork_job(
        &self,
        tx: &mut orch_db::Tx,
        caller: &Caller,
        thread: &threads::ThreadRow,
        draft: Draft,
        batch: Option<Id>,
        preflight: Id,
    ) -> Result<JobRow, PlanError> {
        let now = self.inner.clock.now();
        let conn: &mut orch_db::Conn = &mut *tx;
        let replace = draft.request.result == ForkResult::Replace;
        let copy_worktree = draft.copy_worktree;
        let reach = draft.source_scope.reach;
        let mount_decisions = draft.mount_decisions;
        let plan = draft.plan;
        let from_environment = plan
            .from
            .environment
            .as_ref()
            .map(|environment| environment.id)
            .ok_or_else(|| {
                EngineError::unreadable("a thread that is switched is in an environment")
            })?;
        let to_environment = match &plan.to.environment {
            ForkPlanTarget::Existing(environment) => Some(environment.id),
            ForkPlanTarget::Made { .. } => None,
        };
        let recipe = Recipe {
            preflight_id: preflight,
            result: draft.request.result,
            request: draft.request,
            options: plan.options.clone(),
            source_thread: thread.node_id,
            new_thread: (!replace).then(new_id),
            new_segment: new_id(),
            by: caller.user(),
            from_environment,
            to_environment,
            source_segment: thread.current_segment_id,
            batch_id: batch,
            forkplan: plan,
        };
        let phases = phases_of(&recipe.forkplan);
        let mut plan_json = serde_json::to_value(&recipe)?;
        let operation = new_id();
        let confirmed = source::confirmed(conn, thread, caller, &recipe, operation, reach).await?;
        source::attach(&mut plan_json, &confirmed)?;
        plan_json["copy_worktree"] = json!(copy_worktree);
        plan_json["mount_decisions"] = serde_json::to_value(&mount_decisions)?;
        plan_json["file_caller_token"] = json!(caller.grant().and_then(|grant| grant.token()));
        if let Some(object) = plan_json.as_object_mut() {
            object.insert(
                "origin".to_owned(),
                serde_json::to_value(orch_db::audit::current_origin())?,
            );
            object.insert(
                "from_environment".to_owned(),
                Json::String(from_environment.to_string()),
            );
            if let Some(to) = to_environment {
                object.insert("to_environment".to_owned(), Json::String(to.to_string()));
            }
            let machine = harnesses::machine_of(
                thread.arrangement.as_deref(),
                &resource_rows::provider(conn, thread.provider_id.unwrap_or_default())
                    .await?
                    .ok_or_else(|| EngineError::unreadable("the thread's provider is gone"))?,
                environment_rows::environment(conn, from_environment)
                    .await?
                    .and_then(|environment| environment.machine_id),
            );
            if let Some(machine) = machine {
                object.insert(
                    made::OLD_MACHINE.to_owned(),
                    Json::String(machine.to_string()),
                );
            }
        }
        let progress = serde_json::to_value(initial_progress(&phases))?;
        let job = fork_rows::insert(
            conn,
            &fork_rows::NewForkJob {
                id: operation,
                thread: thread.node_id,
                environment: to_environment.or(Some(from_environment)),
                batch,
                plan: &plan_json,
                progress: &progress,
                created_by: caller.user(),
            },
            now,
        )
        .await?;
        if confirmed.accepted_turn.is_none() && wants_history(&recipe.request) {
            source::capture(conn, job.id, &confirmed, confirmed.range, now).await?;
        }
        if replace {
            // From this transaction on the thread is held: with no turn running it stands blocked at once;
            // with one, it stands blocked when that turn ends.
            let mut unit = Unit::new(conn, now);
            if threads::active_turn(&mut *unit.conn, thread.node_id)
                .await?
                .is_none()
                && matches!(
                    thread.state.as_str(),
                    thread_state::IDLE | thread_state::BLOCKED
                )
            {
                let seq = unit.stamp(thread.node_id).await?;
                crate::notifications::set_state(
                    &mut unit,
                    thread.node_id,
                    &[thread_state::IDLE, thread_state::BLOCKED],
                    &StateChange::blocked(
                        crate::locks::SWITCHING,
                        Some("It is being switched to another provider, profile or environment; what is sent to it waits."),
                    ),
                    seq,
                )
                .await?;
            }
            unit.finish().await?;
        }
        Ok(job)
    }
}

// ---------------------------------------------------------------- the phases

/// A fork or a switch, phase by phase.
pub struct Forking {
    recipe: Recipe,
}

impl Forking {
    /// The work of the job `row`.
    pub fn of(row: &JobRow) -> Result<Self, EngineError> {
        let recipe: Recipe = serde_json::from_value(row.plan.clone()).map_err(|error| {
            EngineError::unreadable(format!("the plan of a fork does not read: {error}"))
        })?;
        Ok(Self { recipe })
    }

    fn replace(&self) -> bool {
        self.recipe.result == ForkResult::Replace
    }

    /// The thread the result is: the one that is switched, or the one that is made.
    fn result_thread(&self) -> Id {
        self.recipe.new_thread.unwrap_or(self.recipe.source_thread)
    }

    fn arrangement_word(&self) -> String {
        serde_json::to_value(self.recipe.forkplan.to.arrangement)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    /// Whether the result lands in an environment that this job makes.
    fn makes_environment(&self) -> bool {
        matches!(
            self.recipe.forkplan.to.environment,
            ForkPlanTarget::Made { .. }
        )
    }
}

#[async_trait]
impl Phased for Forking {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        match phase {
            "lock" => self.lock(job).await,
            "provision" => self.provision(job).await,
            "copy" => self.copy(job).await,
            "history" => self.history(job).await,
            "verify" => self.verify(job).await,
            "commit" => self.commit(job).await,
            "cleanup" => self.cleanup(job).await,
            other => Err(failed(
                INTERNAL,
                format!("a fork has no phase called {other}"),
            )),
        }
    }

    async fn undo(&self, job: &Job, _why: &Stop) -> Undone {
        let undone = self.take_back(job).await;
        if job
            .plan()
            .await
            .is_ok_and(|plan| plan.get("history_target_kept") == Some(&Json::Bool(true)))
        {
            return Undone::Partly("The target keeps actual authored queued words and its files; its exact native preparation remains unavailable until explicitly recovered.".into());
        }
        if job.plan().await.is_ok_and(|plan| {
            plan.get("commit_done").is_none() && plan.get("fork_exact_original").is_some()
        }) {
            // Exact-point copies have their own namespace journal, outside the legacy merge
            // copy receipt. Do not describe those files as removed or recursively erase them.
            return Undone::Partly("the exact saved-file target or stage is kept for recovery; later files are preserved".into());
        }
        match crate::fork_source::cleanup_worktree(job).await {
            Ok(Some(result)) if !result.removed => Undone::Partly(
                result
                    .reason
                    .unwrap_or_else(|| "a changed worktree is kept".into()),
            ),
            Err(_) => Undone::Partly(
                "the original worktree's cleanup awaits its unit; its branch and files are kept"
                    .into(),
            ),
            _ => undone,
        }
    }

    async fn result(&self, _job: &Job) -> Option<Json> {
        Some(json!({ "thread_id": self.result_thread() }))
    }
}

impl Forking {
    // ------------------------------------------------------------ lock

    async fn lock(&self, job: &Job) -> Result<Done, Stop> {
        let engine = job.engine();
        let thread = self.recipe.source_thread;
        if !self.replace() && wants_history(&self.recipe.request) {
            // All on a confirmed empty source still owns an explicit empty cut
            // before any target is created. Missing is never evidence of empty.
            self.source_cut(job).await?;
        }
        if self.replace() {
            engine.cancel_pending_approvals(thread).await?;
            let plan = job.plan().await?;
            let since = match member::<DateTime<Utc>>(&plan, made::WAITING_SINCE) {
                Some(at) => at,
                None => {
                    let now = job.now();
                    job.remember(&json!({ made::WAITING_SINCE: now })).await?;
                    now
                }
            };
            loop {
                job.check()?;
                let active = {
                    let mut conn = engine.db().acquire().await.map_err(EngineError::from)?;
                    threads::active_turn(&mut conn, thread)
                        .await
                        .map_err(EngineError::from)?
                };
                let Some(turn) = active else { break };
                match self.recipe.options.active_turn {
                    ActiveTurn::Interrupt => {
                        engine
                            .interrupt_for_change(thread, &turn, self.recipe.by)
                            .await?;
                    }
                    ActiveTurn::Wait => {
                        let limit = chrono::Duration::minutes(self.recipe.options.wait_minutes);
                        if job.now() - since >= limit {
                            return Err(failed(
                                "wait_timeout",
                                format!(
                                    "the turn did not end within {} minutes; nothing was changed",
                                    self.recipe.options.wait_minutes
                                ),
                            ));
                        }
                    }
                }
                job.progress(0.0, Some("waiting for the turn to end"))
                    .await?;
                job.pause(LOOK_EVERY).await?;
            }
            // With the turn out of the way, the thread stands blocked as `switching`.
            if let Err(error) = engine.preflight(thread).await {
                tracing::warn!(%error, %thread, "a thread that is switched could not be looked at");
            }
        } else if !self.makes_environment() {
            self.make_new_thread(job).await?;
        }
        Ok(Done::Finished)
    }

    /// Makes the thread a fork results in, held like a switched one until the job is done. It is made
    /// as its maker would make it, with the same checks, under the identifier the plan reserved.
    async fn make_new_thread(&self, job: &Job) -> Result<(), Stop> {
        let engine = job.engine();
        let Some(new_thread) = self.recipe.new_thread else {
            return Ok(());
        };
        let confirmed = source::retained(&job.plan().await?)?;
        let (own_params, source_model) = {
            let mut conn = engine.db().acquire().await.map_err(EngineError::from)?;
            if threads::thread(&mut conn, new_thread)
                .await
                .map_err(EngineError::from)?
                .is_some()
            {
                return Ok(());
            }
            // The source's own choices of how its model runs (its context, its speed, its access program), as
            // written at the thread itself and not inherited.
            let own_params = confirmed.own_params.clone();
            let source_model = confirmed.model.clone();
            (own_params, source_model)
        };
        let plan = &self.recipe.forkplan;
        let request = &self.recipe.request;
        let environment = match &plan.to.environment {
            ForkPlanTarget::Existing(environment) => environment.id,
            ForkPlanTarget::Made { .. } => {
                let plan_json = job.plan().await?;
                member::<Id>(&plan_json, made::ENVIRONMENT).ok_or_else(|| {
                    failed(
                        INTERNAL,
                        "the fork's target environment is missing from its job",
                    )
                })?
            }
        };
        let name = request.name.clone().unwrap_or_else(|| {
            let mut name = confirmed.name.clone();
            let room = crate::binding::NAME_MAX_CHARS.saturating_sub(FORK_SUFFIX.chars().count());
            if name.chars().count() > room {
                name = name.chars().take(room).collect();
            }
            format!("{name}{FORK_SUFFIX}")
        });
        let mut settings: Vec<(String, Json)> = Vec::new();
        // They go with a fork that keeps the model; a fork to another model starts from what its place sets, as a
        // switch keeps the thread's own.
        if let Some(params) =
            own_params.filter(|_| source_model.as_deref() == Some(plan.to.model.as_str()))
        {
            settings.push((
                orch_settings_contract::registry::keys::AGENT_PARAMS.to_owned(),
                params,
            ));
        }
        if let Some(cwd) = request.cwd.as_ref().filter(|cwd| !cwd.is_empty()) {
            settings.push((
                orch_settings_contract::registry::keys::THREAD_CWD.to_owned(),
                Json::String(cwd.clone()),
            ));
        }
        // Birth resolves the destination's inherited mode normally, so an offered locked value needs
        // no override. The history snapshot and publication still enforce the confirmed preview.
        let page = fork_page(request.access.as_ref());
        let mut tx = engine.db().begin().await?;
        let caller = source::authorize(&mut tx, job, &self.recipe, &confirmed, None).await?;
        let created = engine
            .create_thread_in_transaction(
                &mut tx,
                &caller,
                &crate::binding::NewThread {
                    topic: confirmed.topic,
                    name,
                    agent: plan.to.agent.id,
                    provider: plan.to.provider.id,
                    environment,
                    directory: plan.to.directory.clone(),
                    arrangement: serde_json::from_value(to_json(&plan.to.arrangement)?).ok(),
                    model: Some(plan.to.model.clone()),
                    effort: plan.to.effort.clone(),
                },
                &crate::binding::Birth {
                    first_input: None,
                    prepare_source: Some(false),
                    worktree: None,
                    titled_by_automation: request.name.is_none(),
                    settings,
                    page,
                },
                new_thread,
                job.now(),
            )
            .await;
        if let Err(error) = created {
            return Err(match error {
                crate::binding::CreateThreadError::Internal(error) => Stop::from(error),
                other => failed("fork_not_made", other.to_string()),
            });
        }
        sqlx::query("INSERT INTO thread_native_guards(thread_id,preparation_job,ready) VALUES($1,$2,false)
            ON CONFLICT(thread_id) DO UPDATE SET preparation_job=EXCLUDED.preparation_job,ready=false
            WHERE thread_native_guards.preparation_job=EXCLUDED.preparation_job")
            .bind(new_thread).bind(job.id()).execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        // It is held from this moment: pre-flight finds the job and blocks it.
        if let Err(error) = engine.preflight(new_thread).await {
            tracing::warn!(%error, thread = %new_thread, "a thread that is being made could not be looked at");
        }
        Ok(())
    }

    // ------------------------------------------------------------ provision

    async fn provision(&self, job: &Job) -> Result<Done, Stop> {
        let environment = self.ensure_environment(job).await?;
        self.ensure_harness(job, &environment).await?;
        if self.recipe.new_thread.is_some() {
            self.make_new_thread(job).await?;
        }
        Ok(Done::Finished)
    }

    /// The environment the result lands in, up: made when the plan said so, started when it is stopped.
    async fn ensure_environment(
        &self,
        job: &Job,
    ) -> Result<environment_rows::EnvironmentRow, Stop> {
        let engine = job.engine();
        let plan_json = job.plan().await?;
        let shared_maker = member::<Id>(&plan_json, "batch_environment_job");
        let shared_environment =
            if let Some(maker) = shared_maker.filter(|maker| *maker != job.id()) {
                let maker = {
                    let mut conn = engine.db().acquire().await.map_err(EngineError::from)?;
                    orch_db::queries::jobs::job(&mut conn, maker)
                        .await
                        .map_err(EngineError::from)?
                        .ok_or_else(|| {
                            failed(
                                "environment_not_made",
                                "the batch's environment-making job is gone",
                            )
                        })?
                };
                Some(match member::<Id>(&maker.plan, made::ENVIRONMENT) {
                    Some(environment) => environment,
                    // A member may fail in its lock phase before provisioning. Another accepted member
                    // then makes the shared target; its own switch remains independent of that failure.
                    None if !maker.is_open() => self.make_environment(job).await?,
                    None => {
                        return Err(failed(
                            "environment_not_made",
                            "the shared environment is missing from its provisioning job",
                        ));
                    }
                })
            } else {
                None
            };
        let id = match self
            .recipe
            .to_environment
            .or(shared_environment)
            .or_else(|| member::<Id>(&plan_json, made::ENVIRONMENT))
        {
            Some(id) => id,
            None => self.make_environment(job).await?,
        };
        if shared_environment.is_some() {
            job.remember(&json!({made::ENVIRONMENT:id})).await?;
        }
        let began = tokio::time::Instant::now();
        let mut asked_to_start = false;
        loop {
            job.check()?;
            let row = {
                let mut conn = engine.db().acquire().await.map_err(EngineError::from)?;
                environment_rows::environment(&mut conn, id)
                    .await
                    .map_err(EngineError::from)?
                    .ok_or_else(|| failed("environment_unavailable", "the environment is gone"))?
            };
            let has_unit = {
                let mut conn = engine.db().acquire().await.map_err(EngineError::from)?;
                orch_db::queries::environment_facts::has_unit(&mut conn, id).await?
            };
            match row.status.as_str() {
                "running" => return Ok(row),
                "failed" => {
                    return Err(failed(
                        "environment_unavailable",
                        row.status_detail
                            .unwrap_or_else(|| "the environment failed".to_owned()),
                    ));
                }
                // An environment this job made is being made by its own job: it is waited for, not started.
                "none" | "stopped" if has_unit && !asked_to_start && !self.makes_environment() => {
                    asked_to_start = true;
                    if let Err(error) = engine.start_environment(id, Some(self.recipe.by)).await {
                        return Err(failed("environment_unavailable", error.to_string()));
                    }
                }
                _ => {}
            }
            if began.elapsed() >= ENVIRONMENT_WAIT {
                return Err(failed(
                    "environment_unavailable",
                    format!(
                        "the environment is {} and did not come up in time",
                        row.status
                    ),
                ));
            }
            job.progress(0.5, Some("waiting for the environment"))
                .await?;
            job.pause(LOOK_EVERY).await?;
        }
    }

    /// Makes the environment a plan asked for, owned by whoever confirmed it, and remembers it so that
    /// the undo can take it away.
    async fn make_environment(&self, job: &Job) -> Result<Id, Stop> {
        let engine = job.engine();
        let Some(ForkEnvironment::New { new }) = &self.recipe.request.environment else {
            return Err(failed(INTERNAL, "the plan names no environment to make"));
        };
        let planned_host = match &self.recipe.forkplan.to.environment {
            ForkPlanTarget::Made { new } => match &new.place {
                orch_api_contract::forks::Place::Host(host) => Some(host.host.id),
                orch_api_contract::forks::Place::Environment(_) => None,
            },
            ForkPlanTarget::Existing(_) => None,
        };
        let now = job.now();
        let mut tx = engine.db().begin().await.map_err(EngineError::from)?;
        let batch_members = if let Some(batch) = self.recipe.batch_id {
            // The same batch lock as claiming and cleanup. No native work occurs in this transaction.
            // Racing followers, including a restarted maker, reuse the one durable creation checkpoint.
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text,0))")
                .bind(batch)
                .execute(&mut *tx)
                .await
                .map_err(orch_db::DbError::from)
                .map_err(EngineError::from)?;
            let members = fork_rows::of_batch(&mut tx, batch)
                .await
                .map_err(EngineError::from)?;
            if let Some(environment) = members
                .iter()
                .find_map(|member_row| member::<Id>(&member_row.plan, made::ENVIRONMENT))
            {
                return Ok(environment);
            }
            members
                .into_iter()
                .map(|member| member.id)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let owned = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        let confirmed = source::retained(&owned.plan)?;
        let caller = source::authorize(&mut tx, job, &self.recipe, &confirmed, None).await?;
        let principal = orch_authz::Principal::load(&mut tx, caller, Time::new(now))
            .await
            .map_err(EngineError::from)?;
        let mut config = new.config.clone().unwrap_or_else(|| json!({}));
        let mount_decisions = crate::mount_copies::of_plan(&owned.plan)?;
        crate::mount_copies::detach_in(&mut config, &mount_decisions.detached);
        let topic = confirmed.topic;
        let name = new
            .name
            .clone()
            .unwrap_or_else(|| crate::environments::DEFAULT_NAME.to_owned());
        let made = crate::environments::create(
            &mut tx,
            now,
            &principal,
            &crate::environments::Request {
                owner: self.recipe.by,
                name: Some(&name),
                home: new.home_node_id.unwrap_or(topic),
                template: new.template_id,
                host: match new.host_id {
                    Some(choice) => Some(choice),
                    None => planned_host.map(Some),
                },
                config: &config,
            },
        )
        .await
        .map_err(|error| failed("environment_not_made", error.to_string()))?;
        if !mount_decisions.copies.is_empty() {
            orch_db::queries::jobs::merge_plan(
                &mut tx,
                made.job.id,
                &json!({"mount_decisions": mount_decisions}),
                now,
            )
            .await?;
        }
        let selectors = crate::volume_carry::carried(self.recipe.forkplan.files.as_ref());
        if !selectors.is_empty() {
            let source = hosting_rows::live_runtime(&mut tx, self.recipe.from_environment)
                .await?
                .ok_or_else(|| {
                    failed(
                        "volume_copy_unavailable",
                        "the reviewed source runtime is gone",
                    )
                })?;
            orch_db::queries::jobs::merge_plan(&mut tx, made.job.id, &json!({"volume_carry": crate::volume_carry::Carry { source_runtime: source.id, selectors }}), now).await?;
        }
        // Registration, its one provisioning job and the maker checkpoint commit together.
        orch_db::queries::jobs::merge_plan(
            &mut tx,
            job.id(),
            &json!({made::ENVIRONMENT:made.id}),
            now,
        )
        .await
        .map_err(EngineError::from)?;
        for member in batch_members {
            orch_db::queries::jobs::merge_plan(
                &mut tx,
                member,
                &json!({"batch_environment_job":job.id()}),
                now,
            )
            .await
            .map_err(EngineError::from)?;
        }
        tx.commit()
            .await
            .map_err(orch_db::DbError::from)
            .map_err(EngineError::from)?;
        job.remember(&json!({ made::ENVIRONMENT: made.id })).await?;
        engine.kick();
        Ok(made.id)
    }

    /// The provider's runtime and the harness of the result are up.
    async fn ensure_harness(
        &self,
        job: &Job,
        environment: &environment_rows::EnvironmentRow,
    ) -> Result<(), Stop> {
        use crate::providers::Keeping;
        let engine = job.engine();
        let plan = &self.recipe.forkplan;
        let (agent, provider, machine) = {
            let mut conn = engine.db().acquire().await.map_err(EngineError::from)?;
            let agent = resource_rows::agent(&mut conn, plan.to.agent.id)
                .await
                .map_err(EngineError::from)?
                .ok_or_else(|| failed("agent_unavailable", "the agent profile is gone"))?;
            let provider = resource_rows::provider(&mut conn, plan.to.provider.id)
                .await
                .map_err(EngineError::from)?
                .ok_or_else(|| failed("provider_unavailable", "the provider is gone"))?;
            let machine = harnesses::machine_of(
                Some(&self.arrangement_word()),
                &provider,
                environment.machine_id,
            )
            .ok_or_else(|| {
                failed(
                    "harness_unavailable",
                    "no machine is there for the harness to run on",
                )
            })?;
            (agent, provider, machine)
        };
        let kind =
            serde_json::from_value(to_json(&plan.to.arrangement)?).map_err(EngineError::from)?;
        let path = crate::binding::credential_path(kind, &provider, &agent);
        if path == orch_agent_contract::credentials::CredentialPath::Forwarded {
            match engine
                .ensure_provider_started(self.result_thread(), provider.id)
                .await?
            {
                Keeping::Up => {}
                Keeping::Failed(outcome) => {
                    return Err(failed("provider_unavailable", format!("{outcome:?}")));
                }
                Keeping::Cannot(error) => {
                    return Err(failed("provider_unavailable", format!("{error:?}")));
                }
            }
        }
        let key = HarnessKey {
            provider: provider.id,
            agent: agent.id,
            machine,
        };
        match engine
            .ensure_harness(key, &agent, ids::agent_start(job.id()))
            .await?
        {
            harnesses::Started::Up => Ok(()),
            harnesses::Started::Failed(outcome) => {
                Err(failed("harness_unavailable", format!("{outcome:?}")))
            }
        }
    }
}

/// The rows of the page a fork is given in its request; a fork that names none starts with the defaults
/// of the place it is made in.
fn fork_page(
    access: Option<&orch_api_contract::access::ThreadAccess>,
) -> Option<orch_access_contract::pages::PageChange> {
    use orch_api_contract::access::{AccessPreset, ThreadAccess};
    match access {
        Some(ThreadAccess::Preset(AccessPreset::Private)) => {
            Some(orch_access_contract::pages::PageChange {
                whole: Some(orch_access_contract::pages::Whole::Private),
                ..orch_access_contract::pages::PageChange::default()
            })
        }
        Some(ThreadAccess::Change(change)) => serde_json::to_value(change)
            .ok()
            .and_then(|value| serde_json::from_value(value).ok()),
        Some(ThreadAccess::As(as_source)) => as_source
            .change
            .as_ref()
            .and_then(|change| serde_json::to_value(change).ok())
            .and_then(|value| serde_json::from_value(value).ok()),
        Some(ThreadAccess::Preset(_)) | None => None,
    }
}

impl Engine {
    /// Stops a turn for a person who confirmed a change that cannot wait for it, and sees that it is
    /// driven to its end: the harness is asked, and the engine ends it by force if it does not answer.
    pub(crate) async fn interrupt_for_change(
        &self,
        thread: Id,
        turn: &threads::TurnRow,
        by: Id,
    ) -> Result<(), EngineError> {
        if turn.interrupted_by.is_none() {
            let mut tx = self.inner.db.begin().await?;
            let finished = {
                let mut unit = Unit::new(&mut tx, self.inner.clock.now());
                if threads::lock(&mut *unit.conn, thread).await?.is_some() {
                    let seq = unit.stamp(thread).await?;
                    threads::set_interrupt_cause(&mut *unit.conn, turn.id, "environment_change")
                        .await?;
                    threads::mark_interrupted_by(&mut *unit.conn, turn.id, by, seq).await?;
                    self.withdraw_interrupted_harness(&mut unit, thread, turn.id)
                        .await?;
                    unit.wake(thread);
                }
                unit.finish().await?
            };
            self.commit(tx, finished).await?;
        }
        self.drive_interrupt(thread).await
    }
}

/// How the files of the source are named to the machine that reads them: the thread's directory, or the
/// selection's paths below it.
pub(crate) fn snapshot_selection(
    directory: &str,
    files: Option<&FileSelection>,
    notes: Option<bool>,
) -> Selection {
    let below = |path: &str| {
        let path = path.trim_matches('/');
        match (directory.is_empty(), path.is_empty()) {
            (true, _) => path.to_owned(),
            (false, true) => directory.to_owned(),
            (false, false) => format!("{directory}/{path}"),
        }
    };
    let notes_path = below(crate::memory::DIRECTORY);
    let mut include = match files {
        None | Some(FileSelection::All) => vec![directory.to_owned()],
        Some(FileSelection::Nothing) => Vec::new(),
        Some(FileSelection::Selection { include, .. }) => {
            include.iter().map(|path| below(path)).collect()
        }
    };
    let mut exclude: Vec<String> = match files {
        Some(FileSelection::Selection {
            exclude: Some(exclude),
            ..
        }) => exclude.iter().map(|path| below(path)).collect(),
        _ => Vec::new(),
    };
    match notes {
        Some(true) if !include.contains(&directory.to_owned()) => include.push(notes_path),
        Some(false) => exclude.push(notes_path),
        _ => {}
    }
    Selection { include, exclude }
}

impl Forking {
    // ------------------------------------------------------------ copy

    async fn copy(&self, job: &Job) -> Result<Done, Stop> {
        if self.recipe.forkplan.worktree.is_some() || self.recipe.forkplan.files_at.is_some() {
            return crate::fork_source::copy(job, &self.recipe).await;
        }
        let selection = match &self.recipe.request.environment {
            Some(ForkEnvironment::New { new }) => new.files.as_ref(),
            Some(ForkEnvironment::Existing { files, .. }) => files.as_ref(),
            _ => None,
        };
        if matches!(selection, Some(FileSelection::Nothing)) {
            // Whole home/private volumes were imported by the new environment's provisioning job.
            return Ok(Done::Skipped(
                "no workspace files were selected; owned volumes were carried before startup"
                    .into(),
            ));
        }
        let engine = job.engine();
        let plan = &self.recipe.forkplan;
        let plan_json = job.plan().await?;
        let target_id = self
            .recipe
            .to_environment
            .or_else(|| member::<Id>(&plan_json, made::ENVIRONMENT))
            .ok_or_else(|| failed(INTERNAL, "the environment the files go to is not known"))?;
        let (source, target) = {
            let mut conn = engine.db().acquire().await.map_err(EngineError::from)?;
            let source = environment_rows::environment(&mut conn, self.recipe.from_environment)
                .await
                .map_err(EngineError::from)?
                .ok_or_else(|| {
                    failed("environment_unavailable", "the source environment is gone")
                })?;
            let target = environment_rows::environment(&mut conn, target_id)
                .await
                .map_err(EngineError::from)?
                .ok_or_else(|| {
                    failed("environment_unavailable", "the target environment is gone")
                })?;
            (source, target)
        };
        let (Some(from_machine), Some(to_machine)) = (source.machine_id, target.machine_id) else {
            return Err(failed(
                "environment_unavailable",
                "an environment the files travel between is on no machine",
            ));
        };
        let source_directory = {
            let mut conn = engine.db().acquire().await.map_err(EngineError::from)?;
            threads::thread(&mut conn, self.recipe.source_thread)
                .await
                .map_err(EngineError::from)?
                .map(|row| row.workspace_dir)
                .unwrap_or_default()
        };
        crate::fork_source::authorize_copy(job, &self.recipe, &source, &target, &source_directory)
            .await?;
        let carries_notes = plan.memory.as_ref().map(|memory| memory.carried);
        let selection = snapshot_selection(
            &source_directory,
            self.recipe_files().as_ref(),
            carries_notes,
        );
        if selection.include.is_empty() {
            return Ok(Done::Skipped("no file was chosen".to_owned()));
        }
        let source_directory_root = plan
            .files
            .as_ref()
            .map(|files| files.path_map.from.as_str())
            .ok_or_else(|| failed(INTERNAL, "the copy's planned source directory is missing"))?;
        let target_directory = crate::sessions::absolute_directory(
            crate::sessions::workspace_root(
                &target.workspace_root,
                &target.facts,
                target.generation,
            ),
            &plan.to.directory,
        );
        if member::<Copied>(&plan_json, made::COPIED).is_none() {
            let made_by_job = self
                .directory_is_new(job, &target, &plan.to.directory)
                .await;
            job.remember(&json!({
                made::COPIED: Copied {
                    environment: target.id,
                    directory: target_directory.clone(),
                    made_by_job,
                }
            }))
            .await?;
        }
        // Once for everything, then once more for what changed on the source meanwhile; what is there
        // already with the right hash is skipped by the machine.
        for pass in 0u32..2 {
            crate::fork_source::authorize_copy(
                job,
                &self.recipe,
                &source,
                &target,
                &source_directory,
            )
            .await?;
            let pass_bytes = pass.to_le_bytes();
            let snapshot = ids::derived(
                "fork.snapshot",
                &[job.id().as_bytes().as_slice(), pass_bytes.as_slice()],
            );
            let independent = plan_json.get("copy_worktree").and_then(Json::as_bool) == Some(true);
            let command = if independent {
                Outgoing::new(
                    snapshot,
                    from_machine,
                    SerialKey::Environment(source.id),
                    &orch_link_contract::source::GitWorktreeSnapshot {
                        environment_id: source.id,
                        job_id: job.id(),
                        directory: source_directory.clone(),
                        selection: selection.clone(),
                    },
                )
                .map_err(EngineError::from)?
            } else {
                Outgoing::new(
                    snapshot,
                    from_machine,
                    SerialKey::Environment(source.id),
                    &FsSnapshot {
                        work: None,
                        environment_id: source.id,
                        job_id: job.id(),
                        selection: selection.clone(),
                    },
                )
                .map_err(EngineError::from)?
            };
            let answer = job
                .command_part(command, f64::from(pass) / 2.0..f64::from(pass) / 2.0 + 0.25)
                .await?;
            let mut taken: FsSnapshotResult = if independent {
                let taken: orch_link_contract::source::GitWorktreeSnapshotResult =
                    serde_json::from_value(answer).map_err(EngineError::from)?;
                if !taken.independent {
                    return Err(failed(
                        "conflict.stale_preflight",
                        "the source is no longer the worktree whose repository was planned",
                    ));
                }
                taken.snapshot
            } else {
                serde_json::from_value(answer).map_err(EngineError::from)?
            };
            {
                let mut conn = engine.db().acquire().await.map_err(EngineError::from)?;
                taken.manifest_blob = crate::selection::rebase_manifest(
                    engine,
                    &mut conn,
                    &taken.manifest_blob,
                    source_directory_root,
                )
                .await?;
            }
            job.progress(
                f64::from(pass) / 2.0 + 0.25,
                Some("the files are read from the source"),
            )
            .await?;
            let write = ids::derived(
                "fork.materialize",
                &[job.id().as_bytes().as_slice(), pass_bytes.as_slice()],
            );
            crate::fork_source::authorize_copy(
                job,
                &self.recipe,
                &source,
                &target,
                &source_directory,
            )
            .await?;
            job.command_part(
                Outgoing::new(
                    write,
                    to_machine,
                    SerialKey::Environment(target.id),
                    &FsMaterialize {
                        work: None,
                        environment_id: target.id,
                        job_id: Some(job.id()),
                        manifest_blob: taken.manifest_blob,
                        target_root: target_directory.clone(),
                        purpose: MaterializePurpose::Move,
                    },
                )
                .map_err(EngineError::from)?,
                f64::from(pass) / 2.0 + 0.25..f64::from(pass) / 2.0 + 0.5,
            )
            .await?;
            if independent {
                job.command(
                    Outgoing::new(
                        ids::derived("fork.worktree.verify", &[job.id().as_bytes(), &pass_bytes]),
                        to_machine,
                        SerialKey::Environment(target.id),
                        &orch_link_contract::source::GitWorktreeStandalone {
                            environment_id: target.id,
                            job_id: job.id(),
                            directory: target_directory.clone(),
                        },
                    )
                    .map_err(EngineError::from)?,
                )
                .await?;
            }
            job.progress(
                f64::from(pass) / 2.0 + 0.5,
                Some("the files are written to the target"),
            )
            .await?;
        }
        Ok(Done::Finished)
    }

    /// The selection of files the request made.
    fn recipe_files(&self) -> Option<FileSelection> {
        match &self.recipe.request.environment {
            Some(ForkEnvironment::Existing { files, .. }) => files.clone(),
            Some(ForkEnvironment::New { new }) => new.files.clone(),
            _ => None,
        }
    }

    /// Whether nothing was in the directory of the target before the job wrote there: what the last list
    /// of its tree knows. A target nobody has listed is taken to have the directory, and so is not
    /// removed again.
    async fn directory_is_new(
        &self,
        job: &Job,
        target: &environment_rows::EnvironmentRow,
        directory: &str,
    ) -> bool {
        if self.makes_environment() {
            return true;
        }
        let engine = job.engine();
        let Ok(mut conn) = engine.db().acquire().await else {
            return false;
        };
        let Ok(Some(row)) =
            orch_db::queries::environments::workspace::environment(&mut conn, target.id).await
        else {
            return false;
        };
        let Some(blob) = row.manifest_blob else {
            return false;
        };
        drop(conn);
        match engine.read_manifest(&blob).await {
            Ok(manifest) => !directory.is_empty() && !manifest.has_below(directory),
            Err(_) => false,
        }
    }
}

// ---------------------------------------------------------------- history

/// The command that opens the session of the result, and what it was opened as.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Attempt {
    method: HistoryMethod,
    mode: SessionMode,
    body: SessionOpen,
    model_inputs: Vec<Id>,
    instruction_sources: Json,
}

impl Forking {
    async fn history(&self, job: &Job) -> Result<Done, Stop> {
        let plan_json = job.plan().await?;
        if member::<Opened>(&plan_json, made::SESSION).is_some() {
            return Ok(Done::Finished);
        }
        let engine = job.engine();
        let plan = &self.recipe.forkplan;
        let planned = plan.history.method;
        let result_thread = self.result_thread();
        let target_id = self
            .recipe
            .to_environment
            .or_else(|| member::<Id>(&plan_json, made::ENVIRONMENT))
            .ok_or_else(|| failed(INTERNAL, "the environment of the result is not known"))?;
        let captured = self.source_cut(job).await?;
        let (environment, agent, provider, counts) = {
            let mut conn = engine.db().acquire().await.map_err(EngineError::from)?;
            let environment = environment_rows::environment(&mut conn, target_id)
                .await?
                .ok_or_else(|| failed("environment_unavailable", "the environment is gone"))?;
            let agent = resource_rows::agent(&mut conn, plan.to.agent.id)
                .await?
                .ok_or_else(|| failed("agent_unavailable", "the agent profile is gone"))?;
            let provider = resource_rows::provider(&mut conn, plan.to.provider.id)
                .await?
                .ok_or_else(|| failed("provider_unavailable", "the provider is gone"))?;
            let counted =
                orch_db::queries::thread_history_cuts::counts(&mut conn, captured.id).await?;
            let counts = fork_history::Counts {
                turns: counted.turns,
                items: counted.items,
                left_out_before: (captured.first_turn > 1).then_some(captured.first_turn),
                bytes: 0,
            };
            (environment, agent, provider, counts)
        };
        let source_ref = captured.source_native_ref.clone();
        let snapshot = {
            let mut conn = engine.db().acquire().await.map_err(EngineError::from)?;
            engine
                .session_snapshot(
                    &mut conn,
                    self,
                    result_thread,
                    &environment,
                    &agent,
                    &provider,
                )
                .await
                .map_err(|error| match error {
                    EngineError::Unreadable(reason) => {
                        let code = if reason == crate::sessions::APPROVAL_UNSUPPORTED {
                            "approval_mode_unsupported"
                        } else if reason == crate::sessions::APPROVAL_NOT_ALLOWED {
                            "approval_mode_not_allowed"
                        } else if reason == APPROVAL_MODE_LOCKED {
                            "approval_mode_locked"
                        } else {
                            "binding_incompatible"
                        };
                        failed(code, reason)
                    }
                    other => Stop::from(other),
                })?
        };
        source::freeze_target(job, &self.recipe, &snapshot).await?;
        recovery::retain_original(job, self).await?;
        let old_machine = member::<Id>(&plan_json, made::OLD_MACHINE);
        let target_can_import_neutral = snapshot.descriptor.capabilities.import_neutral;
        let mut method = planned;
        let mut notes: Vec<String> = Vec::new();
        if method == HistoryMethod::NativeTransplant
            && !snapshot.descriptor.capabilities.import_native
        {
            method = if target_can_import_neutral {
                HistoryMethod::NeutralImport
            } else {
                HistoryMethod::FileHandoff
            };
            notes.push(format!(
                "Native import is unavailable with {} {}; history travels as {method:?}",
                snapshot.descriptor.kind, snapshot.descriptor.harness.version
            ));
        }
        let boundary = if matches!(
            method,
            HistoryMethod::NativeFork | HistoryMethod::NativeTransplant
        ) {
            if let Some(machine) = old_machine {
                fork_history::cut::native_boundary(
                    engine,
                    &captured,
                    machine,
                    plan.from.provider.id,
                    plan.from.agent.id,
                    ids::derived("fork.native-record", &[job.id().as_bytes()]),
                )
                .await?
            } else {
                None
            }
        } else {
            None
        };
        let through = boundary.as_ref().map(|boundary| boundary.ordinal);
        let mut tried: u32 = 0;
        loop {
            job.check()?;
            let outcome = self
                .try_method(
                    job,
                    &snapshot,
                    method,
                    through,
                    source_ref.as_deref(),
                    old_machine,
                    tried,
                )
                .await?;
            match outcome {
                Ok(mut opened) => {
                    let mut report = opened.report.take().unwrap_or_else(|| {
                        fork_history::report_counts(opened.method, &counts, None)
                    });
                    report.warnings.extend(notes);
                    opened.report = Some(report);
                    job.remember(&json!({ made::SESSION: opened })).await?;
                    return Ok(Done::Finished);
                }
                Err(why) => {
                    let next = match method {
                        HistoryMethod::SameSession
                        | HistoryMethod::NativeFork
                        | HistoryMethod::NativeTransplant
                            if target_can_import_neutral =>
                        {
                            HistoryMethod::NeutralImport
                        }
                        HistoryMethod::SameSession
                        | HistoryMethod::NativeFork
                        | HistoryMethod::NativeTransplant
                        | HistoryMethod::NeutralImport => HistoryMethod::FileHandoff,
                        _ => {
                            return Err(failed(
                                "history_not_carried",
                                format!("the session could not be opened with the history: {why}"),
                            ));
                        }
                    };
                    notes.push(format!(
                        "the plan said {method:?}; it could not be done ({why}), and the history travelled as {next:?}"
                    ));
                    method = next;
                    tried += 1;
                }
            }
        }
    }

    /// Opens the session of the result by one method. `Ok(Err(why))`: the harness could not take the
    /// history that way, and the next way is tried.
    #[allow(
        clippy::too_many_arguments,
        reason = "everything one attempt to open a session with a history needs, read once by the caller"
    )]
    async fn try_method(
        &self,
        job: &Job,
        snapshot: &crate::sessions::StartSnapshot,
        method: HistoryMethod,
        through: Option<u32>,
        source_ref: Option<&str>,
        old_machine: Option<Id>,
        attempt: u32,
    ) -> Result<Result<Opened, String>, Stop> {
        use crate::sessions::{SessionImport, session_open_body_importing};
        let engine = job.engine();
        let machine = snapshot.key.machine;
        let result_thread = self.result_thread();
        let mut import = SessionImport::default();
        let mode = match method {
            HistoryMethod::SameSession => {
                let complete = {
                    let mut conn = engine.db().acquire().await?;
                    let cut = orch_db::queries::thread_history_cuts::cut(&mut conn, job.id())
                        .await?
                        .ok_or_else(|| {
                            failed(
                                "history_source_unconfirmed",
                                "The confirmed source cut is absent.",
                            )
                        })?;
                    self.replace()
                        && i64::from(cut.last_turn)
                            == orch_db::queries::thread_history::last_turn(
                                &mut conn,
                                cut.thread_id,
                                cut.continuation_id,
                            )
                            .await?
                };
                if !complete {
                    return Ok(Err(
                        "a whole-session resume cannot trim the confirmed source boundary".into(),
                    ));
                }
                import.native_session_ref = source_ref.map(str::to_owned);
                SessionMode::Resume
            }
            HistoryMethod::NativeFork => {
                if through.is_none() || source_ref.is_none() {
                    return Ok(Err(
                        "the captured source has no exact recorded native turn boundary".into(),
                    ));
                }
                import.native_session_ref = source_ref.map(str::to_owned);
                import.upto_turn_ordinal = through;
                SessionMode::Fork
            }
            HistoryMethod::NativeTransplant => {
                if !self.replace() {
                    return Ok(Err("the native exporter cannot address a captured inactive source while its thread keeps working".into()));
                }
                if through.is_none() {
                    return Ok(Err(
                        "the captured source has no exact recorded native turn boundary".into(),
                    ));
                }
                let Some(from) = old_machine else {
                    return Ok(Err("the source's harness is not known".to_owned()));
                };
                let export = Outgoing::new(
                    ids::derived(
                        "fork.export",
                        &[
                            job.id().as_bytes().as_slice(),
                            attempt.to_le_bytes().as_slice(),
                        ],
                    ),
                    from,
                    SerialKey::Thread(self.recipe.source_thread),
                    &SessionExport {
                        thread_id: self.recipe.source_thread,
                        kind: orch_link_contract::commands::ExportKind::Native,
                        upto_turn_ordinal: through,
                    },
                )
                .map_err(EngineError::from)?;
                let export = source::save(
                    job,
                    &self.recipe,
                    &format!("export_{attempt}"),
                    export,
                    None,
                )
                .await?;
                let exported = match source::run(job, export).await {
                    Ok(value) => value,
                    Err(Stop::Failed(refused)) if refused.code == "unsupported" => {
                        return Ok(Err(refused.message));
                    }
                    Err(other) => return Err(other),
                };
                let exported: SessionExportResult =
                    serde_json::from_value(exported).map_err(EngineError::from)?;
                source::exported(job, &format!("native_export_{attempt}"), &exported).await?;
                import.archive = Some(SessionOpenArchive {
                    format: exported.format,
                    blob: exported.blob,
                    size: exported.size,
                });
                import.upto_turn_ordinal = through;
                SessionMode::ImportNative
            }
            HistoryMethod::NeutralImport => {
                // What the new agent is told is the newest turns that fit the share of its model's window, read from the
                // end of the captured history a page at a time. The plan chose this method because all of it fits; if
                // it no longer does, the next way carries what does not.
                let cut = self.source_cut(job).await?;
                let budget = carry_budget(engine, snapshot)
                    .await?
                    .unwrap_or(fork_history::UNKNOWN_WINDOW_NEUTRAL_BYTES);
                let newest = {
                    let mut conn = engine.db().acquire().await?;
                    fork_history::newest(
                        &mut conn,
                        crate::history_pages::Source::Cut(&cut),
                        (i64::from(cut.first_turn.max(1)), i64::from(cut.last_turn)),
                        i64::from(cut.last_turn),
                        budget,
                    )
                    .await?
                };
                if !newest.complete {
                    return Ok(Err(
                        "the conversation no longer fits the share of the new model's window"
                            .into(),
                    ));
                }
                let text = fork_history::neutral_lines(&newest.selected);
                let (blob, size) =
                    fork_history::cut::keep_bytes(engine, job.id(), "neutral", text.into_bytes())
                        .await?;
                import.history = Some(SessionOpenHistory { blob, size });
                SessionMode::ImportNeutral
            }
            HistoryMethod::SummaryHandoff | HistoryMethod::FileHandoff => {
                self.write_handoff(job, snapshot).await?;
                SessionMode::New
            }
            HistoryMethod::Nothing => SessionMode::New,
        };
        // The old session is closed first when the old harness and the new one are on one machine: a
        // machine keeps one session for a thread. It is resumed by the next turn if this does not go through.
        if self.replace()
            && old_machine == Some(machine)
            && !matches!(
                method,
                HistoryMethod::SameSession | HistoryMethod::NativeFork
            )
        {
            let close = Outgoing::new(
                ids::derived(
                    "fork.close_old",
                    &[
                        job.id().as_bytes().as_slice(),
                        attempt.to_le_bytes().as_slice(),
                    ],
                ),
                machine,
                SerialKey::Thread(result_thread),
                &orch_link_contract::continuations::SessionCloseFenced {
                    thread_id: result_thread,
                    segment_id: self.recipe.source_segment.ok_or_else(|| {
                        failed(
                            "history_source_unconfirmed",
                            "The original close has no captured segment.",
                        )
                    })?,
                },
            )
            .map_err(EngineError::from)?;
            let close =
                source::save(job, &self.recipe, &format!("close_{attempt}"), close, None).await?;
            match source::run(job, close).await {
                Ok(_) => {}
                Err(Stop::Failed(error)) if error.code == "session.segment_changed" => {}
                Err(error) => return Err(error),
            }
            engine.forget_open_sessions(result_thread);
        }
        let mut body = session_open_body_importing(snapshot, mode, &import).map_err(Stop::from)?;
        body.mcp = engine
            .mcp_for_session(snapshot.environment.id, result_thread)
            .await
            .map_err(Stop::from)?;
        body.raw_after = orch_db::queries::raw_events::last_number(
            &mut *engine.inner.db.acquire().await.map_err(EngineError::from)?,
            snapshot.segment.id,
        )
        .await
        .map_err(EngineError::from)?;
        let opening = ids::session_open(self.recipe.new_segment, attempt);
        let saved = job.plan().await?;
        let attempt_record = if let Some(old) = saved
            .get("history_attempts")
            .and_then(|attempts| attempts.get(format!("open_{attempt}")))
        {
            let old: Attempt = serde_json::from_value(old.clone()).map_err(EngineError::from)?;
            if old.method != method || old.mode != mode {
                return Err(failed(
                    "history_command_changed",
                    "The retained history stage has another method.",
                ));
            }
            old
        } else {
            let record = engine
                .record_node_model_input(
                    result_thread,
                    "session",
                    &opening.to_string(),
                    &snapshot.instruction_blocks(),
                )
                .await?;
            let tools = engine
                .record_node_model_input(
                    result_thread,
                    "tool",
                    &format!("{opening}/tools"),
                    &snapshot.tool_blocks(),
                )
                .await?;
            Attempt {
                method,
                mode,
                body,
                model_inputs: vec![record, tools],
                instruction_sources: snapshot.instruction_sources(&[]),
            }
        };
        let predecessor = if self.replace()
            && old_machine == Some(machine)
            && matches!(
                method,
                HistoryMethod::SameSession | HistoryMethod::NativeFork
            ) {
            self.recipe
                .source_segment
                .zip(source_ref)
                .map(
                    |(segment_id, native)| orch_agent_contract::history::NativePrefixSource {
                        thread_id: result_thread,
                        segment_id,
                        native_session_ref: native.to_owned(),
                    },
                )
        } else {
            None
        };
        let command = if method == HistoryMethod::SameSession
            && snapshot
                .descriptor
                .capabilities
                .native_input_prefix
                .is_some()
        {
            let source = orch_agent_contract::history::NativePrefixSource {
                thread_id: result_thread,
                segment_id: self.recipe.source_segment.ok_or_else(|| {
                    failed(
                        "history_source_unconfirmed",
                        "The retained source segment is absent.",
                    )
                })?,
                native_session_ref: source_ref
                    .ok_or_else(|| {
                        failed(
                            "history_source_unconfirmed",
                            "The retained source native identity is absent.",
                        )
                    })?
                    .to_owned(),
            };
            Outgoing::new(
                opening,
                machine,
                SerialKey::Thread(result_thread),
                &orch_link_contract::continuations::SessionContinuationResume {
                    thread_id: result_thread,
                    session: Box::new(attempt_record.body.clone()),
                    source,
                    replace_segment_id: None,
                    replace_retained_source: None,
                },
            )
        } else {
            Outgoing::new(
                opening,
                machine,
                SerialKey::Thread(result_thread),
                &orch_link_contract::continuations::SessionOpenFenced {
                    thread_id: result_thread,
                    session: Box::new(attempt_record.body.clone()),
                    predecessor,
                },
            )
        }
        .map_err(EngineError::from)?;
        let command = source::save(
            job,
            &self.recipe,
            &format!("open_{attempt}"),
            command,
            Some(&attempt_record),
        )
        .await?;
        let opened = match source::run(job, command.clone()).await {
            Ok(value) => {
                for id in &attempt_record.model_inputs {
                    engine
                        .settle_node_model_input(
                            result_thread,
                            *id,
                            crate::model_inputs::Sent::Taken,
                        )
                        .await;
                }
                value
            }
            Err(Stop::Failed(refused)) if refused.code == "unsupported" => {
                for id in &attempt_record.model_inputs {
                    engine
                        .settle_node_model_input(
                            result_thread,
                            *id,
                            crate::model_inputs::Sent::Refused,
                        )
                        .await;
                }
                return Ok(Err(refused.message));
            }
            Err(other) => {
                for id in &attempt_record.model_inputs {
                    engine
                        .settle_node_model_input(
                            result_thread,
                            *id,
                            crate::model_inputs::Sent::Unknown,
                        )
                        .await;
                }
                return Err(other);
            }
        };
        let opened: SessionOpenResult =
            serde_json::from_value(opened).map_err(EngineError::from)?;
        if opened.native_session_ref.is_empty()
            || method == HistoryMethod::SameSession
                && source_ref != Some(opened.native_session_ref.as_str())
            || method == HistoryMethod::NativeFork
                && source_ref == Some(opened.native_session_ref.as_str())
        {
            return Err(failed(
                "history_native_identity",
                "The native opening returned another source or failed to create its distinct child.",
            ));
        }
        source::child_result(job, &command, &opened).await?;
        if matches!(
            method,
            HistoryMethod::NativeFork | HistoryMethod::NativeTransplant
        ) {
            let cut = self.source_cut(job).await?;
            let boundary: fork_history::cut::NativeBoundary =
                serde_json::from_value(cut.artifacts.get("native_boundary").cloned().ok_or_else(
                    || {
                        failed(
                            "history_native_unconfirmed",
                            "The original native boundary witness is unavailable.",
                        )
                    },
                )?)
                .map_err(EngineError::from)?;
            let mut read = Outgoing::new(
                ids::derived("fork.verify-native-child", &[command.id.as_bytes()]),
                machine,
                SerialKey::Harness {
                    provider_id: snapshot.provider.id,
                    agent_id: snapshot.agent.id,
                },
                &orch_link_contract::commands::AgentReadSession {
                    provider_id: snapshot.provider.id,
                    agent_id: snapshot.agent.id,
                    native_session_ref: opened.native_session_ref.clone(),
                    write: true,
                },
            )
            .map_err(EngineError::from)?;
            read.deadline = command.deadline;
            let read = source::save(job, &self.recipe, "verify_child", read, None).await?;
            let child: orch_link_contract::commands::AgentReadSessionResult =
                serde_json::from_value(source::run(job, read).await?).map_err(EngineError::from)?;
            fork_history::cut::verify_native_record(engine, &boundary, &child).await?;
        }
        let report = opened
            .import_report
            .as_ref()
            .and_then(|report| serde_json::to_value(report).ok())
            .and_then(|value| serde_json::from_value::<HistoryReport>(value).ok());
        Ok(Ok(Opened {
            machine,
            thread: result_thread,
            native_session_ref: opened.native_session_ref,
            mode: attempt_record.mode,
            method: attempt_record.method,
            report,
            model_inputs: attempt_record.model_inputs,
            instruction_sources: crate::instruction_sources::merge(
                &attempt_record.instruction_sources,
                &snapshot
                    .instruction_sources(&opened.instruction_sources)
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|source| source["evidence"] == "reported")
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            approval_reviewer: opened.approval_reviewer,
        }))
    }

    /// Writes the conversation to `.orch/handoff/<thread>.md` of the result's directory, where the new
    /// agent is told to read it.
    async fn write_handoff(
        &self,
        job: &Job,
        snapshot: &crate::sessions::StartSnapshot,
    ) -> Result<(), Stop> {
        use crate::manifest::{Entry, EntryKind, Header, MANIFEST_VERSION, build};
        let engine = job.engine();
        let mut cut = self.source_cut(job).await?;
        if cut.artifacts.get("transcript").is_none() {
            // Written to a file a page of turns at a time and put in the store from it: the conversation is never held.
            fork_history::cut::transcript_file(engine, &cut, "transcript").await?;
            cut = self.source_cut(job).await?;
        }
        let transcript_meta =
            cut.artifacts.get("transcript").cloned().ok_or_else(|| {
                failed("history_unreadable", "The captured transcript is absent.")
            })?;
        let transcript_bytes = transcript_meta["size"]
            .as_i64()
            .filter(|size| *size >= 0)
            .ok_or_else(|| failed("history_unreadable", "The transcript size is invalid."))?;
        let transcript_turns = match transcript_meta["turns"].as_i64() {
            Some(turns) => turns,
            // A transcript kept before its turns were counted with it.
            None => {
                let mut conn = engine.db().acquire().await?;
                orch_db::queries::thread_history_cuts::counts(&mut conn, cut.id)
                    .await?
                    .turns
            }
        };
        let wants_summary =
            self.recipe.request.history.as_ref().is_some_and(|history| {
                history.mode == orch_api_contract::forks::HistoryMode::Summary
            });
        // A summary is asked of the place's producer once. Without a producer, when it fails, or when an earlier
        // request has no retained outcome (another paid request is never guessed), the switch goes on: the last
        // turns, a few lines each, stand in for the summary, and the whole transcript is in the file.
        if wants_summary
            && cut.artifacts.get("summary_result").is_none()
            && cut.artifacts.get("summary_skipped").is_none()
        {
            let configured = {
                let mut conn = engine.db().acquire().await?;
                engine.summary_available(&mut conn, cut.thread_id).await?
            };
            let recovery_known = {
                let mut conn = engine.db().acquire().await?;
                crate::calls::read_workflow(
                    &mut conn,
                    cut.thread_id,
                    orch_provider_contract::calls::Purpose::Summary,
                    job.id(),
                )
                .await?
                .is_some()
            };
            if cut.artifacts.get("summary_started").is_some() && !recovery_known || !configured {
                let reason = if configured {
                    "unconfirmed"
                } else {
                    "none_configured"
                };
                source::artifact(
                    job,
                    &self.recipe,
                    "summary_skipped",
                    &json!({"reason": reason}),
                    true,
                )
                .await?;
            } else {
                let choice = if let Some(choice) = cut.artifacts.get("summary_choice") {
                    choice.clone()
                } else {
                    let choice = {
                        let mut conn = engine.db().acquire().await?;
                        crate::summaries::capture_handoff_choice(
                            &mut conn,
                            cut.thread_id,
                            cut.head["name"].as_str(),
                        )
                        .await?
                    };
                    source::artifact(job, &self.recipe, "summary_choice", &choice, false).await?;
                    choice
                };
                source::artifact(
                    job,
                    &self.recipe,
                    "summary_started",
                    &json!({"request_id":job.id(),"transcript":transcript_meta}),
                    false,
                )
                .await?;
                // The helper gets brief turns and older index lines within its own window's share.
                let summary_budget =
                    crate::summaries::handoff_budget(engine, cut.thread_id, &choice).await?;
                let previous =
                    fork_history::cut::summary_input(engine, &cut, &summary_budget).await?;
                let mut previous_recovery = None;
                let summary = loop {
                    job.check()?;
                    let made = crate::summaries::handoff_captured(
                        engine,
                        cut.thread_id,
                        &previous,
                        job.id(),
                        &choice,
                    )
                    .await;
                    if made.is_ok() {
                        break made;
                    }
                    let recovery = {
                        let mut conn = engine.db().acquire().await?;
                        crate::calls::read_workflow(
                            &mut conn,
                            cut.thread_id,
                            orch_provider_contract::calls::Purpose::Summary,
                            job.id(),
                        )
                        .await?
                    };
                    let Some(recovery) = recovery.filter(|recovery| {
                        recovery.context["workflow_id"] == json!(job.id())
                            && recovery.state == "scheduled"
                    }) else {
                        break made;
                    };
                    let Some(due) = recovery.due_at else {
                        break made;
                    };
                    if previous_recovery == Some(recovery.reference) && due <= job.now() {
                        break made;
                    }
                    previous_recovery = Some(recovery.reference);
                    // The ordinary job feed publishes this retained due time before the cancellable pause.
                    job.progress(0.0, Some("calls.serviceScheduled")).await?;
                    let pause = (due - job.now()).to_std().unwrap_or_default();
                    job.pause(pause).await?;
                };
                match summary {
                    Ok(summary) => {
                        source::artifact(
                            job,
                            &self.recipe,
                            "summary_result",
                            &json!({"text":summary,"at":Time::new(job.now())}),
                            true,
                        )
                        .await?;
                    }
                    Err(error) => {
                        tracing::warn!(%error, job = %job.id(), "the switch's summary failed; the last turns stand in");
                        source::artifact(
                            job,
                            &self.recipe,
                            "summary_skipped",
                            &json!({"reason": "failed"}),
                            true,
                        )
                        .await?;
                    }
                }
            }
            cut = self.source_cut(job).await?;
        }
        let summary = cut
            .artifacts
            .get("summary_result")
            .filter(|_| wants_summary)
            .map(|result| {
                Ok::<_, Stop>((
                    result["text"].as_str().ok_or_else(|| {
                        failed("history_unreadable", "The retained summary is invalid.")
                    })?,
                    serde_json::from_value::<Time>(result["at"].clone())
                        .map_err(EngineError::from)?,
                ))
            })
            .transpose()?;
        // What goes into the new agent's context beside the file: with a summary, nothing more; otherwise, where
        // the new agent imports nothing, the most recent turns word for word, as many as fill the place's share of
        // the new model's window; for a Summary without one, or a model whose window is not known, the last turns
        // briefly.
        let budget = if summary.is_some() || wants_summary {
            None
        } else {
            carry_budget(engine, snapshot).await?
        };
        let carried = if summary.is_some() {
            None
        } else {
            let mut conn = engine.db().acquire().await?;
            match budget {
                Some(budget) => {
                    fork_history::cut::recent(
                        &mut conn,
                        &cut,
                        crate::written::Detail::Normal,
                        Some(budget),
                        None,
                    )
                    .await?
                }
                None => {
                    fork_history::cut::recent(
                        &mut conn,
                        &cut,
                        crate::written::Detail::Brief,
                        None,
                        Some(SUMMARY_STAND_IN_TURNS),
                    )
                    .await?
                }
            }
        };
        let carried_at = Time::new(cut.captured_at);
        let notice = crate::summaries::handoff_notice(
            cut.thread_id,
            transcript_turns,
            usize::try_from(transcript_bytes).unwrap_or(usize::MAX),
            summary,
            carried.as_ref().map(|carried| (carried, carried_at)),
        );
        source::artifact(job, &self.recipe, "handoff_notice", &json!(notice), false).await?;
        job.remember(&json!({"handoff_text":notice})).await?;
        let transcript_blob =
            serde_json::from_value(transcript_meta["blob"].clone()).map_err(EngineError::from)?;
        let transcript_size = transcript_bytes;
        let root = format!("{}/.orch", snapshot.directory().trim_end_matches('/'));
        let manifest = if let Some(meta) = cut.artifacts.get("handoff_manifest") {
            // Readback verifies bytes and hash before using the retained manifest.
            fork_history::cut::read_bytes(engine, meta).await?;
            serde_json::from_value(meta["blob"].clone()).map_err(EngineError::from)?
        } else {
            let now = cut.captured_at;
            let written = build(
                &Header {
                    v: MANIFEST_VERSION,
                    root: root.clone(),
                    generated_at: Time::new(now),
                    generation: i64::from(snapshot.environment.generation),
                    case_sensitive: true,
                },
                vec![Entry {
                    path: fork_history::handoff_path(cut.thread_id),
                    kind: EntryKind::File,
                    size: transcript_size,
                    mtime_ns: now.timestamp_nanos_opt().unwrap_or(0),
                    mode: 0o644,
                    hash: Some(transcript_blob),
                    link: None,
                }],
            )
            .map_err(|error| failed(INTERNAL, error.to_string()))?;
            fork_history::cut::keep_bytes(engine, cut.id, "handoff_manifest", written.bytes)
                .await?
                .0
        };
        let Some(machine) = snapshot.environment.machine_id else {
            return Err(failed(
                "environment_unavailable",
                "the environment is on no machine",
            ));
        };
        let command = Outgoing::new(
            ids::derived("fork.handoff", &[job.id().as_bytes(), manifest.as_bytes()]),
            machine,
            SerialKey::Environment(snapshot.environment.id),
            &FsMaterialize {
                work: None,
                environment_id: snapshot.environment.id,
                job_id: None,
                manifest_blob: manifest,
                target_root: root,
                purpose: MaterializePurpose::Delivery,
            },
        )
        .map_err(EngineError::from)?;
        let command = source::save(job, &self.recipe, "handoff", command, None).await?;
        source::run(job, command).await?;
        Ok(())
    }
}

impl Engine {
    /// Forgets which sessions of a thread this process saw opened: the next turn opens them again.
    pub(crate) fn forget_open_sessions(&self, thread: Id) {
        crate::engine::lock(&self.inner.sessions).forget_thread_sessions(thread);
    }

    /// What a session of the result is opened from, set up as it will be after the change: the thread's
    /// row with the binding of the target, its created fork segment (or the future switch segment), and the stand-ins for the turn,
    /// the input and the lease that the opening of a session asks of a turn that is starting.
    #[allow(
        clippy::too_many_lines,
        reason = "the same reading of the database as the start of a turn makes, for what the thread will be"
    )]
    pub(crate) async fn session_snapshot(
        &self,
        conn: &mut orch_db::Conn,
        forking: &Forking,
        thread_id: Id,
        environment: &environment_rows::EnvironmentRow,
        agent: &resource_rows::AgentRow,
        provider: &resource_rows::ProviderRow,
    ) -> Result<crate::sessions::StartSnapshot, EngineError> {
        use crate::binding;
        use crate::sessions::{StartSnapshot, confinement};
        use orch_agent_contract::descriptor::OrchToolsSupport;
        let recipe = &forking.recipe;
        let plan = &recipe.forkplan;
        let now = self.inner.clock.now();
        let mut thread = threads::thread(conn, thread_id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the thread to open a session for is gone"))?;
        // A fork was created through the ordinary birth path, which chose its real first segment.
        // The recipe's new_segment is the switch segment/attempt nonce, not that fork's foreign key.
        let target_segment = if recipe.new_thread.is_some() {
            thread
                .current_segment_id
                .ok_or_else(|| EngineError::unreadable("the fork has no first segment"))?
        } else {
            recipe.new_segment
        };
        let descriptor = harnesses::descriptor_of(agent)?
            .ok_or_else(|| EngineError::unreadable("the profile never reported"))?;
        let mut settings = crate::resolved::load(conn, thread_id).await?;
        let call_candidate =
            crate::calls::switch_candidate(conn, thread_id, provider.id, agent.id, &plan.to.model)
                .await?;
        if let Some(candidate) = &call_candidate
            && let (Some(params), Some(extra)) = (
                settings.params.as_object_mut(),
                candidate.params.as_object(),
            )
        {
            params.extend(extra.clone());
        }
        let arrangement = forking
            .arrangement_on(conn, &descriptor, &settings, environment, provider)
            .await?;
        thread.agent_id = Some(agent.id);
        thread.provider_id = Some(provider.id);
        thread.environment_id = Some(environment.id);
        thread.workspace_dir.clone_from(&plan.to.directory);
        thread.arrangement = Some(arrangement.kind.as_str().to_owned());
        thread.executor.clone_from(&arrangement.executor);
        thread.tool_set.clone_from(&arrangement.tool_set);
        let machine_id = harnesses::machine_of(
            thread.arrangement.as_deref(),
            provider,
            environment.machine_id,
        )
        .ok_or_else(|| EngineError::unreadable("no machine is there for the harness"))?;
        let descriptor = harnesses::descriptor_at(conn, agent, provider.id, Some(machine_id))
            .await?
            .unwrap_or(descriptor);
        let descriptor: orch_agent_contract::descriptor::AgentDescriptor = serde_json::from_value(
            crate::hosting::with_build_limits(conn, serde_json::to_value(descriptor)?).await?,
        )?;
        if let Some(mode) = &plan.to.approval_mode {
            let mode = serde_json::from_value(Json::String(mode.clone()))?;
            if !descriptor.approval_modes.contains(&mode) {
                return Err(EngineError::unreadable(
                    crate::sessions::APPROVAL_UNSUPPORTED,
                ));
            }
            if !settings.allowed_modes.contains(&mode) {
                return Err(EngineError::unreadable(
                    crate::sessions::APPROVAL_NOT_ALLOWED,
                ));
            }
            if mode != settings.approval_mode {
                let setting = orch_settings::resolve(
                    conn,
                    orch_settings::Place::Node(thread_id),
                    orch_settings_contract::registry::keys::APPROVAL_MODE,
                )
                .await
                .map_err(EngineError::from)?;
                if !setting.open_to_write() {
                    return Err(EngineError::unreadable(APPROVAL_MODE_LOCKED));
                }
            }
            settings.approval_mode = mode;
        }
        let machine = resource_rows::machine_place(conn, machine_id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the machine of the harness is gone"))?;
        let template = environment_rows::template_spec(conn, environment.id).await?;
        let environment_machine = orch_db::queries::environment_facts::of(conn, environment.id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the environment is gone"))?;
        let (approval, sandbox) = confinement(
            &settings,
            &descriptor,
            environment,
            template.as_ref(),
            &environment_machine,
            arrangement.executor.as_deref(),
        )
        .map_err(EngineError::unreadable)?;
        let model = resource_rows::model(conn, provider.id, &plan.to.model).await?;
        let parameters = crate::binding::model_parameters(model.as_ref().ok_or_else(|| {
            EngineError::unreadable("The confirmed target model is no longer available.")
        })?)?;
        let path = binding::selected_path(&arrangement, provider, agent);
        if let Some(candidate) = &call_candidate {
            use orch_agent_contract::{descriptor::Placement, parameters::Eligibility};
            let context = Eligibility {
                credential_path: path,
                provider_kind: binding::provider_kind(provider),
                environment_placement: if environment
                    .facts
                    .pointer("/environment_descriptor/lifecycle")
                    .and_then(Json::as_str)
                    == Some("unit")
                {
                    Placement::Managed
                } else {
                    Placement::Connected
                },
                model: parameters.clone(),
                harness_version: None,
            };
            if !crate::calls::parameters_ready(
                &descriptor,
                &context,
                &settings,
                &candidate.params,
                true,
            ) {
                return Err(EngineError::unreadable(
                    "The configured candidate parameters are unavailable.",
                ));
            }
        }
        let params = crate::preflight::sessions_params(
            &descriptor,
            &arrangement,
            provider,
            agent,
            environment,
            &parameters,
            &settings,
            path,
        );
        if let Some(why) = self
            .bypass_environment(conn, environment.id, approval)
            .await?
        {
            return Err(EngineError::unreadable(why));
        }
        let available_models = if approval == orch_agent_contract::descriptor::ApprovalMode::Auto {
            self.review_models(conn, &thread, provider.id, Some(recipe.by), &settings)
                .await?
        } else {
            vec![plan.to.model.clone()]
        };
        crate::approval_review::reviewer_allowed(
            approval,
            crate::harnesses::descriptor_of(agent)?.as_ref(),
            &available_models,
        )
        .map_err(EngineError::unreadable)?;
        let topic_name = threads::node_name(conn, thread.topic_id)
            .await?
            .unwrap_or_default();
        let thread_name = threads::node_name(conn, thread_id)
            .await?
            .unwrap_or_default();
        let standing = crate::sessions::standing_of(conn, &settings.standing).await?;
        let neighbours =
            threads::topic_threads_in_environment(conn, thread.topic_id, environment.id, thread_id)
                .await?;
        let other_work =
            threads::other_topic_threads_in_environment(conn, thread.topic_id, environment.id)
                .await?
                > 0;
        let tools = crate::tools::offered_for_session(
            conn,
            &thread,
            descriptor.capabilities.orch_tools == OrchToolsSupport::Mcp,
            self.inner.clock.time(),
        )
        .await?;
        let view = self.environment_capabilities(conn, environment.id).await?;
        let isolation = view
            .as_ref()
            .and_then(|view| view.isolation.clone())
            .filter(|isolation| isolation.runtime.is_some());
        let capabilities = match view {
            Some(view) if view.applies => Some(crate::instructions::CapabilityLine {
                held: view.held.iter().map(|held| held.key.clone()).collect(),
                offered: tools
                    .contains(&orch_agent_contract::events::OrchTool::RequestCapability)
                    .then(|| {
                        view.offered
                            .iter()
                            .map(|offered| offered.key.clone())
                            .collect()
                    }),
            }),
            _ => None,
        };
        let segment = threads::SegmentRow {
            id: target_segment,
            thread_id,
            continuation_id: thread.active_continuation_id,
            ordinal: 0,
            agent_id: agent.id,
            provider_id: provider.id,
            environment_id: environment.id,
            env_generation: environment.generation,
            arrangement: arrangement.kind.as_str().to_owned(),
            executor: arrangement.executor.clone(),
            tool_set: arrangement.tool_set.clone(),
            credential_path: binding::credential_path_word(path).to_owned(),
            native_session_ref: None,
            started_by: Some(recipe.by),
            started_at: now,
            ended_at: None,
            end_reason: None,
            import_fidelity: None,
            import_report: None,
            seq_updated: 0,
        };
        // A session is opened before any turn of its segment exists: the turn and the input are stand-ins
        // that carry the model and the effort; the lease is one for a ticket that is never used, the first
        // turn opens the session again with its own.
        let turn = threads::TurnRow {
            id: Id::nil(),
            thread_id,
            segment_id: target_segment,
            ordinal: 0,
            state: threads::turn_state::STARTING.to_owned(),
            input_id: Id::nil(),
            before_snapshot_id: None,
            files_end_manifest: None,
            end_manifest_blob: None,
            model: plan.to.model.clone(),
            effort: plan.to.effort.clone(),
            params,
            lease_id: None,
            native_turn_ref: None,
            error: None,
            usage: None,
            usage_detail: None,
            context: None,
            limit_bypass: None,
            phase: None,
            phase_at: None,
            waiting_since: None,
            waited_ms: 0,
            interrupted_by: None,
            command_cleanup: None,
            started_at: None,
            completed_at: None,
            created_at: now,
            seq_updated: 0,
            plan: false,
        };
        let input = threads::InputRow {
            id: Id::nil(),
            thread_id,
            continuation_id: thread.active_continuation_id,
            author_id: Some(recipe.by),
            source: "system".to_owned(),
            content: json!([]),
            native_action: None,
            delivery: "queue".to_owned(),
            state: threads::input_state::PENDING.to_owned(),
            state_detail: None,
            limit_bypass: None,
            position: "0".to_owned(),
            turn_id: None,
            client_id: None,
            created_at: now,
            delivered_at: None,
            cancelled_by: None,
            seq_updated: 0,
            attachment_issues: json!([]),
            run_as: None,
            submission_authority: None,
            run_as_authority: None,
            approval_id: None,
            queue_held: false,
            plan: false,
        };
        let lease =
            (path == orch_agent_contract::credentials::CredentialPath::Forwarded).then(|| {
                threads::LeaseRow {
                    id: ids::derived("fork.lease", &[recipe.new_segment.as_bytes().as_slice()]),
                    provider_id: provider.id,
                    thread_id,
                    turn_id: None,
                    entry_id: None,
                    mode: "forwarded".to_owned(),
                    form: None,
                    allowed_models: available_models.clone(),
                    allowance: None,
                    opened_at: now,
                    closed_at: None,
                    close_reason: None,
                }
            });
        Ok(StartSnapshot {
            material: self.topic_material(conn, thread_id).await?,
            thread,
            pending_notice_positions: Vec::new(),
            turn,
            input,
            segment,
            provider: provider.clone(),
            agent: agent.clone(),
            environment: environment.clone(),
            descriptor,
            settings,
            key: HarnessKey {
                provider: provider.id,
                agent: agent.id,
                machine: machine_id,
            },
            machine,
            model,
            path,
            lease,
            arrangement,
            approval,
            available_models,
            sandbox,
            topic_name,
            thread_name,
            standing,
            neighbours,
            other_work,
            author_name: None,
            authors: 0,
            tools,
            capabilities,
            isolation,
            browser_line: crate::tools::browser::preamble(conn, environment.id).await?,
            workspace_configuration: crate::trust::answer_for(
                conn,
                environment.id,
                &agent.harness_kind,
            )
            .await?,
            // The session a fork opens begins no input of its own: nothing is cut before it, and no name begins.
            cut: None,
            unnamed_before: Vec::new(),
            // The session the fork opens is given where its agent is with its instructions.
            context_sent: Json::Null,
        })
    }
}

// ---------------------------------------------------------------- verify, commit, cleanup

/// The words an import fidelity has in the table of segments.
fn fidelity_word(method: HistoryMethod) -> Option<&'static str> {
    match method {
        HistoryMethod::SameSession | HistoryMethod::Nothing => None,
        HistoryMethod::NativeFork | HistoryMethod::NativeTransplant => Some("lossless"),
        HistoryMethod::NeutralImport | HistoryMethod::FileHandoff => Some("structured"),
        HistoryMethod::SummaryHandoff => Some("summary"),
    }
}

impl Forking {
    /// The arrangement the result takes on `environment` as it stands now, by the rule of the plan and by
    /// what the environment has reported. An environment that was made for the result reported nothing
    /// when the plan was made, and its executor and tool set are the ones it has now.
    async fn arrangement_on(
        &self,
        conn: &mut orch_db::Conn,
        descriptor: &orch_agent_contract::descriptor::AgentDescriptor,
        settings: &crate::resolved::Resolved,
        environment: &environment_rows::EnvironmentRow,
        provider: &resource_rows::ProviderRow,
    ) -> Result<orch_agent_contract::arrangement::Arrangement, EngineError> {
        use crate::binding::{self, Facts};
        let offers = binding::environment_offers(conn, environment).await?;
        let machine_list = binding::machine_arrangements_of(conn, provider).await?;
        let profile = resource_rows::agent(conn, self.recipe.forkplan.to.agent.id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the agent profile is gone"))?;
        let facts = Facts {
            descriptor,
            provider,
            environment,
            offers: offers.offers,
            settings,
            entry: None,
            colocated: offers.colocated,
            runtime: binding::runtime_facts(conn, &profile, provider, environment).await?,
        };
        let kind: orch_agent_contract::descriptor::ArrangementType =
            serde_json::from_value(serde_json::to_value(self.recipe.forkplan.to.arrangement)?)?;
        binding::choose_with_machine(&facts, machine_list, Some(kind))
            .map_err(|why| EngineError::unreadable(binding::describe(&why)))
    }
}

impl Forking {
    async fn verify(&self, job: &Job) -> Result<Done, Stop> {
        let limit = Duration::from_secs(
            u64::try_from(self.recipe.options.verify_timeout_secs).unwrap_or(0),
        );
        match tokio::time::timeout(limit, self.checks(job)).await {
            Ok(result) => result,
            Err(_elapsed) => Err(failed(
                "verify_timeout",
                format!(
                    "the checks did not finish within {} seconds",
                    self.recipe.options.verify_timeout_secs
                ),
            )),
        }
    }

    /// The new side is there: the environment runs and its machine is online, the provider is ready, and
    /// the session holds the history.
    async fn checks(&self, job: &Job) -> Result<Done, Stop> {
        let engine = job.engine();
        let plan_json = job.plan().await?;
        let target_id = self
            .recipe
            .to_environment
            .or_else(|| member::<Id>(&plan_json, made::ENVIRONMENT))
            .ok_or_else(|| failed(INTERNAL, "the environment of the result is not known"))?;
        let mut conn = engine.db().acquire().await.map_err(EngineError::from)?;
        let environment = environment_rows::environment(&mut conn, target_id)
            .await
            .map_err(EngineError::from)?
            .ok_or_else(|| failed("verify_failed", "the environment is gone"))?;
        if environment.status != "running" {
            return Err(failed(
                "verify_failed",
                format!("the environment is {}, not running", environment.status),
            ));
        }
        if let Some(machine) = environment.machine_id {
            let online = resource_rows::machine_place(&mut conn, machine)
                .await
                .map_err(EngineError::from)?
                .is_some_and(|place| place.online);
            if !online {
                return Err(failed(
                    "verify_failed",
                    "the machine of the environment is offline",
                ));
            }
        }
        // An environment made for the result was planned from what its template promised: what it offers now
        // decides, and a plan it cannot hold stops here, before anything is committed.
        if self.makes_environment() {
            let offers = crate::binding::environment_offers(&mut conn, &environment).await?;
            let to = &self.recipe.forkplan.to;
            if let Some(executor) = to.isolation.executor.as_deref()
                && !offers.offers.iter().any(|offered| offered == executor)
            {
                return Err(failed(
                    "verify_failed",
                    format!(
                        "the environment made does not offer {executor}, which the plan relied on"
                    ),
                ));
            }
            if self.arrangement_word() == "together"
                && !offers.colocated.contains(&to.agent.harness_kind)
            {
                return Err(failed(
                    "verify_failed",
                    "the environment made does not let the harness run beside its tools, which the plan relied on",
                ));
            }
        }
        let provider = resource_rows::provider(&mut conn, self.recipe.forkplan.to.provider.id)
            .await
            .map_err(EngineError::from)?
            .ok_or_else(|| failed("verify_failed", "the provider is gone"))?;
        if provider.status != "ready" {
            return Err(failed(
                "verify_failed",
                format!("the provider is {}", provider.status),
            ));
        }
        if self.recipe.forkplan.history.method != HistoryMethod::Nothing
            && member::<Opened>(&plan_json, made::SESSION).is_none()
        {
            return Err(failed(
                "verify_failed",
                "the session is not open on the new side",
            ));
        }
        Ok(Done::Finished)
    }

    // ------------------------------------------------------------ commit

    async fn commit(&self, job: &Job) -> Result<Done, Stop> {
        let engine = job.engine();
        let plan_json = job.plan().await?;
        if plan_json.get("commit_done").is_some() {
            return Ok(Done::Finished);
        }
        let opened: Option<Opened> = member(&plan_json, made::SESSION);
        let target_id = self
            .recipe
            .to_environment
            .or_else(|| member::<Id>(&plan_json, made::ENVIRONMENT))
            .ok_or_else(|| failed(INTERNAL, "the environment of the result is not known"))?;
        let now = job.now();
        let mut tx = engine.db().begin().await.map_err(EngineError::from)?;
        let owned = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        if let Some(reason) = crate::jobs::runner::abort_reason(&owned)? {
            return Err(reason);
        }
        let confirmed = source::retained(&owned.plan)?;
        let target = threads::thread(&mut tx, self.result_thread()).await?;
        source::authorize(&mut tx, job, &self.recipe, &confirmed, target.as_ref()).await?;
        if let Some(target) = owned.plan.get("history_target") {
            source::check_target(
                &mut tx,
                &self.recipe,
                &serde_json::from_value::<source::Target>(target.clone())
                    .map_err(EngineError::from)?,
            )
            .await?;
        }
        if !orch_db::queries::jobs::pass_commit(&mut tx, job.id(), now).await? {
            return Err(Stop::Cancelled);
        }
        let finished = {
            let mut unit = Unit::new(&mut tx, now);
            let segment = if self.replace() {
                self.commit_switch(&mut unit, job, target_id, opened.as_ref(), &plan_json)
                    .await?;
                self.recipe.new_segment
            } else {
                self.commit_fork(&mut unit, job, target_id, opened.as_ref(), &plan_json)
                    .await?
            };
            if let Some(opened) = &opened {
                engine
                    .attach_model_inputs(
                        &mut unit,
                        self.result_thread(),
                        segment,
                        &opened.model_inputs,
                    )
                    .await?;
                if opened.instruction_sources.is_array() {
                    let seq = unit.stamp(self.result_thread()).await?;
                    threads::set_instruction_sources(
                        &mut *unit.conn,
                        segment,
                        &opened.instruction_sources,
                        seq,
                    )
                    .await
                    .map_err(EngineError::from)?;
                }
                sqlx::query("UPDATE segments SET approval_reviewer=$2 WHERE id=$1")
                    .bind(segment)
                    .bind(
                        opened
                            .approval_reviewer
                            .as_ref()
                            .map(serde_json::to_value)
                            .transpose()
                            .map_err(EngineError::from)?,
                    )
                    .execute(&mut *unit.conn)
                    .await
                    .map_err(orch_db::DbError::from)
                    .map_err(EngineError::from)?;
            }
            if let Some(target) = self.recipe.new_thread {
                let changed=sqlx::query("UPDATE thread_native_guards SET ready=true WHERE thread_id=$1 AND preparation_job=$2")
                    .bind(target).bind(job.id()).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?.rows_affected();
                if changed != 1 {
                    return Err(failed(
                        "history_target_changed",
                        "Another preparation owns the target's queued work.",
                    ));
                }
            }
            if !wants_history(&self.recipe.request) {
                sqlx::query("INSERT INTO thread_native_guards(thread_id,empty_segment_id) VALUES($1,$2)
                    ON CONFLICT(thread_id) DO UPDATE SET empty_segment_id=EXCLUDED.empty_segment_id")
                    .bind(self.result_thread()).bind(segment).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
            }
            // Readable copy and every native/file consumer now own their result.
            // The release trigger refuses any still-unconfirmed original use.
            orch_db::queries::thread_history_cuts::release(unit.conn, job.id()).await?;
            orch_db::queries::jobs::merge_plan(
                &mut *unit.conn,
                job.id(),
                &json!({ "commit_done": true }),
                now,
            )
            .await
            .map_err(EngineError::from)?;
            unit.finish().await.map_err(Stop::from)?
        };
        engine.commit(tx, finished).await.map_err(Stop::from)?;
        if self.replace() {
            engine.forget_open_sessions(self.recipe.source_thread);
        }
        Ok(Done::Finished)
    }

    /// The switch, in the one transaction: the old segment ends, the new begins with what the session
    /// said of the history, the thread is on the new side, and its history says so.
    async fn commit_switch(
        &self,
        unit: &mut Unit<'_>,
        job: &Job,
        target_id: Id,
        opened: Option<&Opened>,
        remembered: &Json,
    ) -> Result<(), Stop> {
        let plan = &self.recipe.forkplan;
        let thread_id = self.recipe.source_thread;
        let by = self.recipe.by;
        let now = unit.now();
        let thread = threads::lock(&mut *unit.conn, thread_id)
            .await
            .map_err(EngineError::from)?
            .ok_or_else(|| failed("source_gone", "the thread is gone"))?;
        let agent = resource_rows::agent(&mut *unit.conn, plan.to.agent.id)
            .await
            .map_err(EngineError::from)?
            .ok_or_else(|| failed("agent_unavailable", "the agent profile is gone"))?;
        let provider = resource_rows::provider(&mut *unit.conn, plan.to.provider.id)
            .await
            .map_err(EngineError::from)?
            .ok_or_else(|| failed("provider_unavailable", "the provider is gone"))?;
        let environment = environment_rows::environment(&mut *unit.conn, target_id)
            .await
            .map_err(EngineError::from)?
            .ok_or_else(|| failed("environment_unavailable", "the environment is gone"))?;
        let kind: orch_agent_contract::descriptor::ArrangementType =
            serde_json::from_value(to_json(&plan.to.arrangement)?).map_err(EngineError::from)?;
        let path = crate::binding::credential_path(kind, &provider, &agent);
        // What carries out the tool calls is what the environment has now, which for one that was made for
        // this switch is not what the plan could say of it.
        let (executor, tool_set) = {
            let descriptor = harnesses::descriptor_of(&agent)
                .map_err(Stop::from)?
                .ok_or_else(|| failed("agent_unavailable", "the profile never reported"))?;
            let settings = crate::resolved::load(&mut *unit.conn, thread_id).await?;
            let arrangement = self
                .arrangement_on(
                    &mut *unit.conn,
                    &descriptor,
                    &settings,
                    &environment,
                    &provider,
                )
                .await
                .map_err(|error| failed("binding_incompatible", error.to_string()))?;
            (arrangement.executor, arrangement.tool_set)
        };
        let seq = unit.stamp(thread_id).await?;
        let same_runtime = plan.from.agent.id == plan.to.agent.id
            && plan.from.provider.id == plan.to.provider.id
            && thread.arrangement.as_deref() == Some(kind.as_str());
        if let Some(old) = thread.current_segment_id {
            let reason = if same_runtime {
                "environment_moved"
            } else {
                "switched"
            };
            threads::end_segment(&mut *unit.conn, old, reason, now, seq)
                .await
                .map_err(EngineError::from)?;
        }
        threads::insert_segment(
            &mut *unit.conn,
            &NewSegment {
                id: self.recipe.new_segment,
                thread_id,
                agent_id: agent.id,
                provider_id: provider.id,
                environment_id: environment.id,
                env_generation: environment.generation,
                arrangement: kind.as_str(),
                executor: executor.as_deref(),
                tool_set: tool_set.as_deref(),
                credential_path: crate::binding::credential_path_word(path),
                native_session_ref: opened.map(|opened| opened.native_session_ref.as_str()),
                started_by: Some(by),
                started_at: now,
            },
            seq,
        )
        .await
        .map_err(EngineError::from)?;
        let report = self.final_report(opened);
        if let Some(opened) = opened {
            orch_db::queries::machine_streams::bind_segment(
                &mut *unit.conn,
                thread_id,
                self.recipe.new_segment,
                opened.machine,
            )
            .await
            .map_err(EngineError::from)?;
        }
        let report_value = report_json(&report);
        forking::set_segment_import(
            &mut *unit.conn,
            self.recipe.new_segment,
            opened.and_then(|opened| fidelity_word(opened.method)),
            Some(&report_value),
            seq,
        )
        .await
        .map_err(EngineError::from)?;
        // The actions allowed for this thread were allowed on what it ran on: a switch to another provider, profile,
        // environment or directory ends them, each with its line.
        let before = crate::approval_rules::binding_of(&mut *unit.conn, &thread).await?;
        if before.as_ref().is_none_or(|before| {
            before.agent_id != agent.id
                || before.provider_id != provider.id
                || before.environment_id != environment.id
                || before.directory != plan.to.directory
        }) {
            crate::approval_rules::end_for_binding_change(unit, &thread).await?;
        }
        threads::set_binding(
            &mut *unit.conn,
            thread_id,
            &Binding {
                agent: agent.id,
                provider: provider.id,
                arrangement: kind.as_str(),
                executor: executor.as_deref(),
                tool_set: tool_set.as_deref(),
            },
            seq,
        )
        .await
        .map_err(EngineError::from)?;
        threads::set_environment(
            &mut *unit.conn,
            thread_id,
            environment.id,
            &plan.to.directory,
            seq,
        )
        .await
        .map_err(EngineError::from)?;
        threads::set_current(
            &mut *unit.conn,
            thread_id,
            Some(self.recipe.new_segment),
            None,
            seq,
        )
        .await
        .map_err(EngineError::from)?;
        self.write_model_settings(unit, thread_id).await?;
        let reference_time = orch_types::Time::new(unit.now());
        let marker = orch_api_contract::threads::MarkerPayload::Switched(
            orch_api_contract::threads::MarkerSwitched {
                from: orch_api_contract::threads::MarkerSwitchedFrom {
                    agent: plan.from.agent.clone(),
                    provider: plan.from.provider.clone(),
                    environment: plan.from.environment.clone(),
                    directory: thread.workspace_dir.clone(),
                },
                to: orch_api_contract::threads::MarkerSwitchedTo {
                    agent: plan.to.agent.clone(),
                    provider: plan.to.provider.clone(),
                    environment: serde_json::from_value(
                        crate::views::environment_ref_json(
                            &mut *unit.conn,
                            &environment,
                            reference_time,
                        )
                        .await?,
                    )
                    .map_err(EngineError::from)?,
                    directory: plan.to.directory.clone(),
                },
                report: report.clone(),
                by: serde_json::from_value(crate::views::user_ref_json(&mut *unit.conn, by).await?)
                    .map_err(EngineError::from)?,
            },
        );
        crate::ingest::add_marker(
            unit,
            thread_id,
            self.recipe.new_segment,
            None,
            &serde_json::to_value(&marker).map_err(EngineError::from)?,
            &format!("switched:{}", job.id()),
        )
        .await?;
        self.queue_handoff_input(unit, job, remembered, thread_id, opened, seq)
            .await?;
        self.audit(
            unit,
            orch_db::audit::action!(AuditAction::ThreadSwitched),
            thread_id,
            environment.id,
            "switched a thread",
        )
        .await
    }

    /// The fork, in the one transaction: the thread that was made is bound for good, its first segment
    /// holds what the session said of the history, and both threads say so.
    async fn commit_fork(
        &self,
        unit: &mut Unit<'_>,
        job: &Job,
        target_id: Id,
        opened: Option<&Opened>,
        remembered: &Json,
    ) -> Result<Id, Stop> {
        let plan = &self.recipe.forkplan;
        let source_id = self.recipe.source_thread;
        let new_id_ = self
            .recipe
            .new_thread
            .ok_or_else(|| failed(INTERNAL, "a fork has a thread it makes"))?;
        let by = self.recipe.by;
        // The two threads are locked in the order of their identifiers, so that two forks of one thread
        // and a fork of its fork cannot wait for each other.
        let (first, second) = if source_id < new_id_ {
            (source_id, new_id_)
        } else {
            (new_id_, source_id)
        };
        let parent = threads::thread(unit.conn, source_id)
            .await?
            .and_then(|source| source.side_of);
        let mut to_lock = vec![first, second];
        to_lock.extend(parent);
        to_lock.sort();
        to_lock.dedup();
        for thread in to_lock {
            threads::lock(unit.conn, thread).await?;
        }
        let mut seqs = std::collections::BTreeMap::new();
        for thread in [first, second] {
            seqs.insert(thread, unit.stamp(thread).await?);
        }
        let new_seq = seqs[&new_id_];
        let now = unit.now();
        let thread = threads::thread(&mut *unit.conn, new_id_)
            .await
            .map_err(EngineError::from)?
            .ok_or_else(|| failed("fork_gone", "the thread that was made is gone"))?;
        let source = threads::thread(&mut *unit.conn, source_id)
            .await
            .map_err(EngineError::from)?
            .ok_or_else(|| failed("source_gone", "the thread to fork is gone"))?;
        let confirmed = source::retained(remembered)?;
        orch_db::queries::source::set_links(&mut *unit.conn, new_id_, &confirmed.links, new_seq)
            .await?;
        if let Some(info) = remembered.get("fork_source_info") {
            orch_db::queries::source::set_source(
                &mut *unit.conn,
                new_id_,
                &json!({ "info": info }),
                new_seq,
            )
            .await?;
            let mut entry = orch_db::audit::Entry::new(
                orch_db::audit::Actor::User(by),
                orch_db::audit::name!("workspace.worktree_created"),
                "environment",
                Some(target_id),
                "Created the fork's own worktree",
            );
            entry.node_id = Some(new_id_);
            entry.org_id = Some(thread.org_id);
            entry.environment_id = Some(target_id);
            entry.history = true;
            entry.details = json!({ "job_id": job.id(), "directory": plan.to.directory, "branch": info.get("branch"), "base": info.get("head") });
            orch_db::audit::append(
                &mut *unit.conn,
                &entry,
                &orch_db::audit::Origin::default(),
                now,
            )
            .await?;
        }
        let segment = thread
            .current_segment_id
            .ok_or_else(|| failed("fork_gone", "the thread that was made has no segment"))?;
        let environment = environment_rows::environment(&mut *unit.conn, target_id)
            .await
            .map_err(EngineError::from)?
            .ok_or_else(|| failed("environment_unavailable", "the environment is gone"))?;
        // The thread was made in the environment the plan names (one that was made for it is made first) and
        // has been held since, so that nothing moved it.
        if thread.environment_id != Some(environment.id) {
            return Err(failed(
                INTERNAL,
                "the thread that was made is not in the environment that was planned",
            ));
        }
        if let Some(opened) = opened {
            threads::set_native_session_ref(
                &mut *unit.conn,
                segment,
                &opened.native_session_ref,
                new_seq,
            )
            .await
            .map_err(EngineError::from)?;
        }
        let mut report = self.final_report(opened);
        let keep_side = if source.side_of.is_some() {
            let queued:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_inputs WHERE thread_id=$1 AND state IN ('pending','delivering'))")
                .bind(source_id).fetch_one(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
            source.active_continuation_id == confirmed.view
                && source.execution_revision == confirmed.execution_revision
                && source.current_segment_id == confirmed.segment
                && source.current_turn_id.is_none()
                && source.queue_revision == confirmed.queue_revision
                && !queued
        } else {
            false
        };
        if source.side_of.is_some() && !keep_side {
            report.warnings.push("The captured history was forked; the side stays open because its execution or queued words changed during preparation.".into());
        }
        forking::set_segment_import(
            &mut *unit.conn,
            segment,
            opened.and_then(|opened| fidelity_word(opened.method)),
            opened.map(|_| report_json(&report)).as_ref(),
            new_seq,
        )
        .await
        .map_err(EngineError::from)?;
        let captured = if wants_history(&self.recipe.request) {
            Some(
                orch_db::queries::thread_history_cuts::cut(unit.conn, job.id())
                    .await?
                    .ok_or_else(|| {
                        failed(
                            "history_source_unconfirmed",
                            "The confirmed full-history cut is absent.",
                        )
                    })?,
            )
        } else {
            None
        };
        let last_turn = if let Some(cut) = &captured {
            orch_db::queries::thread_history_cuts::turns(
                unit.conn,
                cut.id,
                i64::from(cut.last_turn),
                i64::from(cut.last_turn),
            )
            .await?
            .first()
            .map_or_else(Id::nil, |turn| turn.id)
        } else {
            Id::nil()
        };
        if let Some(captured) = &captured {
            // Only a native fork promises the same item identities in the carried session.
            // Converted imports and handoff files may use the same spelling for entirely new items.
            let replay_segment =
                if opened.is_some_and(|opened| opened.method == HistoryMethod::NativeFork) {
                    captured.source_segment_id
                } else {
                    None
                };
            fork_history::cut::copy(unit, captured, new_id_, segment, replay_segment).await?;
        }
        let by_ref: orch_api_contract::users::UserRef =
            serde_json::from_value(crate::views::user_ref_json(&mut *unit.conn, by).await?)
                .map_err(EngineError::from)?;
        let made_from = orch_api_contract::threads::MarkerPayload::ForkedFrom(
            orch_api_contract::threads::MarkerForkedFrom {
                thread: plan.source.clone(),
                turn_id: last_turn,
                report: report.clone(),
                by: by_ref.clone(),
            },
        );
        crate::ingest::add_marker(
            unit,
            new_id_,
            segment,
            None,
            &serde_json::to_value(&made_from).map_err(EngineError::from)?,
            &format!("forked_from:{}", job.id()),
        )
        .await?;
        if let Some(source_segment) = confirmed.segment {
            let made = orch_api_contract::threads::MarkerPayload::Forked(
                orch_api_contract::threads::MarkerForked {
                    into: orch_api_contract::nodes::NodeRef {
                        thread_display: None,
                        id: new_id_,
                        kind: orch_api_contract::nodes::NodeKind::Thread,
                        name: Some(thread.name.clone()),
                        redacted: false,
                    },
                    turn_id: last_turn,
                    by: by_ref,
                },
            );
            crate::ingest::add_marker(
                unit,
                source_id,
                source_segment,
                None,
                &serde_json::to_value(&made).map_err(EngineError::from)?,
                &format!("forked:{}", job.id()),
            )
            .await?;
        }
        self.write_model_settings(unit, new_id_).await?;
        self.queue_handoff_input(unit, job, remembered, new_id_, opened, new_seq)
            .await?;
        self.carry_page_of_source(unit, source_id, new_id_, by)
            .await?;
        // A side thread that turns out to be worth keeping is forked in the ordinary way, and is closed
        // with the reason that it was kept.
        if keep_side {
            job.engine()
                .settle_side_in(unit.conn, source_id, Some(by), Time::new(now))
                .await?;
            forking::close_with_marker(
                &mut *unit.conn,
                source_id,
                "kept",
                Some(by),
                Some(&forking::KeptAs {
                    thread: new_id_,
                    name: thread.name.clone(),
                }),
                now,
            )
            .await
            .map_err(EngineError::from)?;
            unit.stamp(source_id).await?;
        }
        self.audit(
            unit,
            orch_db::audit::action!(AuditAction::ThreadForked),
            source_id,
            environment.id,
            "forked a thread",
        )
        .await?;
        Ok(segment)
    }

    /// The report that says how the history travelled: the harness's own where it gave one, else what the
    /// plan worked out for the method that was used.
    fn final_report(&self, opened: Option<&Opened>) -> HistoryReport {
        match opened {
            Some(opened) => opened
                .report
                .clone()
                .unwrap_or_else(|| fork_history::report(opened.method, &Selected::default(), None)),
            None => self.recipe.forkplan.history.report.clone(),
        }
    }

    /// The verified handoff accompanies the next real input, including one released from a held queue.
    async fn queue_handoff_input(
        &self,
        unit: &mut Unit<'_>,
        job: &Job,
        plan: &Json,
        thread: Id,
        opened: Option<&Opened>,
        seq: i64,
    ) -> Result<(), Stop> {
        if !opened.is_some_and(|opened| {
            matches!(
                opened.method,
                HistoryMethod::SummaryHandoff | HistoryMethod::FileHandoff
            )
        }) {
            return Ok(());
        }
        let notice = plan
            .get("handoff_text")
            .and_then(Json::as_str)
            .ok_or_else(|| {
                failed(
                    "history_unreadable",
                    "the verified handoff notice is unavailable",
                )
            })?;
        let _row = threads::thread(&mut *unit.conn, thread)
            .await
            .map_err(EngineError::from)?
            .ok_or_else(|| failed("thread_unreadable", "the target thread is unavailable"))?;
        let id = format!("handoff:{}", job.id());
        threads::notices::append(
            &mut *unit.conn,
            thread,
            &json!({"id":id,"kind":"handoff","text":notice}),
        )
        .await
        .map_err(EngineError::from)?;
        threads::set_seq_state(&mut *unit.conn, thread, seq)
            .await
            .map_err(EngineError::from)?;
        Ok(())
    }

    /// Writes the model, effort and confirmed approval mode through the ordinary settings writer.
    /// The preview choice is saved only when the new binding commits.
    async fn write_model_settings(&self, unit: &mut Unit<'_>, thread: Id) -> Result<(), Stop> {
        use orch_db::audit::{Actor, Origin};
        use orch_settings::{Target, write};
        use orch_settings_contract::registry::keys;
        let plan = &self.recipe.forkplan;
        let resolved = crate::resolved::load(&mut *unit.conn, thread).await?;
        let now = unit.now();
        if let Some(mode) = &plan.to.approval_mode {
            let value = Json::String(mode.clone());
            let mode = serde_json::from_value(value.clone())
                .map_err(|error| failed("approval_mode_unsupported", error.to_string()))?;
            // No-history jobs skip session opening, so publication must validate the target too.
            let agent = resource_rows::agent(&mut *unit.conn, plan.to.agent.id)
                .await
                .map_err(EngineError::from)?
                .ok_or_else(|| failed("agent_unavailable", "the agent profile is gone"))?;
            let descriptor = harnesses::descriptor_of(&agent)?
                .ok_or_else(|| failed("agent_unavailable", "the profile never reported"))?;
            if !descriptor.approval_modes.contains(&mode) {
                return Err(failed(
                    "approval_mode_unsupported",
                    crate::sessions::APPROVAL_UNSUPPORTED,
                ));
            }
            if !resolved.allowed_modes.contains(&mode) {
                return Err(failed(
                    "approval_mode_not_allowed",
                    crate::sessions::APPROVAL_NOT_ALLOWED,
                ));
            }
            if value != to_json(&resolved.approval_mode)? {
                write::write(
                    &mut *unit.conn,
                    &write::Write {
                        actor: Actor::User(self.recipe.by),
                        target: Target::Node(thread),
                        key: keys::APPROVAL_MODE,
                        value,
                        locked: None,
                        expected_revision: None,
                        may_lock: false,
                        origin: &Origin::default(),
                    },
                    now,
                )
                .await
                .map_err(|error| match error {
                    orch_settings::write::WriteError::Locked { .. }
                    | orch_settings::write::WriteError::Lock(_) => {
                        failed("approval_mode_locked", APPROVAL_MODE_LOCKED)
                    }
                    other => failed(INTERNAL, other.to_string()),
                })?;
            }
        }
        for (key, value, current) in [
            (
                keys::AGENT_MODEL,
                Some(plan.to.model.clone()),
                resolved.model.clone(),
            ),
            (
                keys::AGENT_EFFORT,
                plan.to.effort.clone(),
                resolved.effort.clone(),
            ),
        ] {
            let Some(value) = value else { continue };
            if current.as_deref() == Some(value.as_str()) {
                continue;
            }
            write::write(
                &mut *unit.conn,
                &write::Write {
                    actor: Actor::User(self.recipe.by),
                    target: Target::Node(thread),
                    key,
                    value: Json::String(value),
                    locked: None,
                    expected_revision: None,
                    may_lock: false,
                    origin: &Origin::default(),
                },
                now,
            )
            .await
            .map_err(|error| failed(INTERNAL, error.to_string()))?;
        }
        Ok(())
    }

    /// The line the history of the thread gets in the record that is kept of what people do.
    async fn audit(
        &self,
        unit: &mut Unit<'_>,
        action: AuditAction,
        thread: Id,
        environment: Id,
        sentence: &str,
    ) -> Result<(), Stop> {
        use orch_db::audit::{self, Actor, Entry, Origin};
        let mut entry = Entry::new(
            Actor::User(self.recipe.by),
            action.as_str(),
            "thread",
            Some(thread),
            sentence,
        );
        entry.history = action.is_history_line();
        entry.node_id = Some(thread);
        entry.environment_id = Some(environment);
        entry.details = json!({
            "from_agent": self.recipe.forkplan.from.agent.id,
            "to_agent": self.recipe.forkplan.to.agent.id,
            "from_provider": self.recipe.forkplan.from.provider.id,
            "to_provider": self.recipe.forkplan.to.provider.id,
            "result": self.recipe.result,
            "job": self.recipe.preflight_id,
        });
        let now = unit.now();
        audit::append(&mut *unit.conn, &entry, &Origin::default(), now)
            .await
            .map_err(EngineError::from)?;
        Ok(())
    }

    /// A fork that starts as the thread it was taken from, which is what a request that names no page gets:
    /// every row that was changed on that page is given to the new thread, and every row that followed the
    /// defaults follows those of the place where the fork is made. Its owners are its maker's alone, and the
    /// owners of the original are never carried. A request that asks for the defaults of the place, for a
    /// private page or for rows of its own carries nothing: those are given to the thread as it is made.
    async fn carry_page_of_source(
        &self,
        unit: &mut Unit<'_>,
        source: Id,
        new: Id,
        by: Id,
    ) -> Result<(), Stop> {
        use orch_access_contract::permissions::Permission;
        use orch_api_contract::access::ThreadAccess;
        use orch_db::queries::access::{self, NewEntry, PageKey, RowWrite};
        let from = match &self.recipe.request.access {
            None => source,
            Some(ThreadAccess::As(as_page)) => as_page.source,
            Some(_) => return Ok(()),
        };
        let now = unit.now();
        let (from_page, new_page) = (
            PageKey::new("node", from, "live"),
            PageKey::new("node", new, "live"),
        );
        let owners = Permission::AccessOwn.as_str();
        for row in access::stored_rows_of(&mut *unit.conn, &from_page)
            .await
            .map_err(EngineError::from)?
            .into_iter()
            .filter(|row| row.permission != owners)
        {
            access::write_row(
                &mut *unit.conn,
                &RowWrite {
                    page: &new_page,
                    profile: &row.profile,
                    permission: &row.permission,
                    inherit: row.inherit,
                    locked: row.locked,
                },
                Some(by),
                now,
            )
            .await
            .map_err(EngineError::from)?;
        }
        for entry in access::entries_of(&mut *unit.conn, &from_page)
            .await
            .map_err(EngineError::from)?
            .into_iter()
            .filter(|entry| entry.permission != owners)
        {
            access::insert_entry(
                &mut *unit.conn,
                &NewEntry {
                    id: new_id(),
                    page: &new_page,
                    profile: &entry.profile,
                    permission: &entry.permission,
                    holder_kind: &entry.holder_kind,
                    holder_id: entry.holder_id,
                    holder_role: entry.holder_role.as_deref(),
                    options: &entry.options,
                    expires_at: entry.expires_at,
                    note: &entry.note,
                    source: orch_access_contract::pages::EntrySource::Creation.as_str(),
                    created_by: Some(by),
                    at: now,
                },
            )
            .await
            .map_err(EngineError::from)?;
        }
        access::bump_epoch(&mut *unit.conn)
            .await
            .map_err(EngineError::from)?;
        Ok(())
    }
}

fn report_json(report: &HistoryReport) -> Json {
    serde_json::to_value(report).unwrap_or(Json::Null)
}

// ---------------------------------------------------------------- cleanup and undoing

impl Forking {
    /// What was left behind is dealt with. Nothing here fails the job: after the commit nothing is undone,
    /// and what could not be done now is done by the reaper's next look.
    async fn cleanup(&self, job: &Job) -> Result<Done, Stop> {
        let engine = job.engine();
        let plan_json = job.plan().await?;
        let plan = &self.recipe.forkplan;
        let opened: Option<Opened> = member(&plan_json, made::SESSION);
        let thread = self.recipe.source_thread;
        // The old session is closed, unless the same session goes on.
        let same_session = opened
            .as_ref()
            .is_some_and(|opened| opened.method == HistoryMethod::SameSession);
        if let Some(old) = member::<Id>(&plan_json, made::OLD_MACHINE)
            && !same_session
            && opened.as_ref().map(|opened| opened.machine) != Some(old)
        {
            let close = {
                let mut conn = engine.db().acquire().await.map_err(EngineError::from)?;
                crate::commands::session_close(
                    &mut conn,
                    ids::derived("fork.close_old_session", &[job.id().as_bytes().as_slice()]),
                    old,
                    thread,
                    self.recipe.source_segment,
                )
                .await?
            };
            if let Err(error) = engine.inner.commands.issue(close).await {
                tracing::warn!(%error, %thread, "the old session of a switched thread could not be closed");
            }
        }
        // The environment the thread left, when it stands empty and may be removed.
        if let Some(left) = plan
            .leaves
            .as_ref()
            .and_then(|leaves| leaves.environment.as_ref())
            && left.action == orch_api_contract::forks::LeftAction::Remove
        {
            let still_used = {
                let mut conn = engine.db().acquire().await.map_err(EngineError::from)?;
                let others = threads::tools::threads_in_environment(&mut conn, left.environment.id)
                    .await
                    .map_err(EngineError::from)?;
                others.into_iter().any(|other| other != thread)
            };
            if !still_used {
                let now = job.now();
                let mut tx = engine.db().begin().await.map_err(EngineError::from)?;
                orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
                let requester = job.row().await?.created_by;
                match engine
                    .schedule_environment_removal(&mut tx, left.environment.id, requester, now, now)
                    .await
                {
                    Err(error) => {
                        tracing::warn!(%error, "the removal of an environment a switch left could not be scheduled")
                    }
                    Ok(false) => {}
                    Ok(true) => {
                        let environment = hosting_rows::environment(&mut tx, left.environment.id)
                            .await?
                            .ok_or_else(|| {
                                EngineError::unreadable("the switched environment is gone")
                            })?;
                        let promise = orch_db::queries::environments::retained::removal(
                            &mut tx,
                            environment.id,
                        )
                        .await?
                        .ok_or_else(|| EngineError::unreadable("the switched removal is gone"))?;
                        let summary = format!(
                            "Removal of {} was scheduled after the thread switched away",
                            environment.name
                        );
                        let mut entry = orch_db::audit::Entry::new(
                            requester
                                .map_or(orch_db::audit::Actor::System, orch_db::audit::Actor::User),
                            "environment.removal_scheduled",
                            "environment",
                            Some(environment.id),
                            &summary,
                        );
                        entry.history = true;
                        entry.node_id = Some(environment.home_node_id);
                        entry.org_id = Some(environment.org_id);
                        entry.environment_id = Some(environment.id);
                        entry.details = json!({"reason":"switch","job":job.id(),"removal_revision":promise.removal_revision,"keep_days":promise.removed_keep_days,"at":now});
                        let origin = plan_json
                            .get("origin")
                            .cloned()
                            .and_then(|value| serde_json::from_value(value).ok())
                            .unwrap_or_default();
                        orch_db::audit::append(&mut tx, &entry, &origin, now).await?;
                        tx.commit()
                            .await
                            .map_err(orch_db::DbError::from)
                            .map_err(EngineError::from)?;
                        engine.kick();
                    }
                }
            }
        }
        Ok(Done::Finished)
    }

    /// Takes back what the job made, before its commit: the session it opened, the files it wrote, the
    /// environment it made, the thread it made. The lock goes with the job.
    async fn take_back(&self, job: &Job) -> Undone {
        let engine = job.engine();
        let Ok(plan_json) = job.plan().await else {
            return Undone::Partly("the job's plan could not be read".to_owned());
        };
        if plan_json.get("commit_done").is_some() {
            return Undone::Everything;
        }
        if let Err(error) = recovery::undo(job, self).await {
            let detail = match error {
                Stop::Lost => "the original history recovery owner changed",
                _ => {
                    "the exact native history outcome or original recovery is still awaiting its owned receipt"
                }
            };
            return Undone::Deferred(detail.to_owned());
        }
        let keep_target = if let Some(target) = self.recipe.new_thread {
            let checked=async {
                let mut tx=engine.db().begin().await?;
                job.lock_owned(&mut tx).await?.ok_or_else(||EngineError::unreadable("The target retention owner changed."))?;
                threads::lock(&mut tx,target).await?;
                let authored:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_inputs WHERE thread_id=$1 AND author_id IS NOT NULL)")
                    .bind(target).fetch_one(&mut *tx).await.map_err(orch_db::DbError::from)?;
                if authored {orch_db::queries::jobs::merge_plan(&mut tx,job.id(),&json!({"history_target_kept":true}),job.now()).await?;}
                tx.commit().await.map_err(orch_db::DbError::from)?;Ok::<_,EngineError>(authored)
            }.await;
            match checked {Ok(kept)=>kept,Err(_)=>return Undone::Deferred("The born target's authored queue cannot yet be checked; no files or resource were removed.".into())}
        } else {
            false
        };
        let mut left: Vec<String> = Vec::new();
        if let Some(copied) = member::<Copied>(&plan_json, made::COPIED)
            && !keep_target
            && copied.made_by_job
            && !(member::<Id>(&plan_json, "batch_environment_job").is_some()
                && self.recipe.batch_id.is_some()
                && engine
                    .batch_keeps_directory(
                        self.recipe.batch_id.unwrap_or_default(),
                        job.id(),
                        copied.environment,
                        &copied.directory,
                    )
                    .await
                    .unwrap_or(true))
        {
            let machine = {
                match engine.db().acquire().await {
                    Ok(mut conn) => environment_rows::environment(&mut conn, copied.environment)
                        .await
                        .ok()
                        .flatten()
                        .and_then(|environment| environment.machine_id),
                    Err(_) => None,
                }
            };
            if let Some(machine) = machine {
                let remove = Outgoing::new(
                    ids::derived("fork.undo_files", &[job.id().as_bytes().as_slice()]),
                    machine,
                    SerialKey::Environment(copied.environment),
                    &FsRemove {
                        environment_id: copied.environment,
                        path: copied.directory.clone(),
                        recursive: true,
                    },
                );
                match remove {
                    Ok(remove) => {
                        if job.command_to_undo(remove).await.is_err() {
                            left.push(format!(
                                "the directory {} that the files were written to could not be removed",
                                copied.directory
                            ));
                        }
                    }
                    Err(error) => left.push(error.to_string()),
                }
            }
        }
        if let Some(environment) = member::<Id>(&plan_json, made::ENVIRONMENT)
            && !keep_target
            && member::<Id>(&plan_json, "batch_environment_job").is_none()
        {
            let now = job.now();
            match engine.db().begin().await {
                Ok(mut tx) => {
                    if orch_db::queries::transaction_guard::lock_authority(&mut tx)
                        .await
                        .is_err()
                        || orch_db::queries::jobs::merge_plan(
                            &mut tx,
                            job.id(),
                            &json!({"scratch_cleanup":environment}),
                            now,
                        )
                        .await
                        .is_err()
                        || hosting_rows::schedule_removal(&mut tx, environment, now, now)
                            .await
                            .is_err()
                        || tx.commit().await.is_err()
                    {
                        left.push(
                            "the environment that was made could not be scheduled for removal"
                                .to_owned(),
                        );
                    } else {
                        engine.kick();
                    }
                }
                Err(_) => left.push(
                    "the environment that was made could not be scheduled for removal".to_owned(),
                ),
            }
        }
        if let Some(new_thread) = self.recipe.new_thread
            && !keep_target
            && let Err(error) = engine
                .delete_thread_made_by_job(new_thread, self.recipe.by)
                .await
        {
            left.push(format!(
                "the thread that was made could not be deleted: {error}"
            ));
        }
        if left.is_empty() {
            Undone::Everything
        } else {
            Undone::Partly(left.join("; "))
        }
    }
}

impl Engine {
    /// Deletes the thread a fork was making when the job is undone: it never had content.
    pub(crate) async fn delete_thread_made_by_job(
        &self,
        thread: Id,
        by: Id,
    ) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let now = self.inner.clock.now();
        let present = threads::lock(&mut tx, thread).await?.is_some();
        let retained:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_inputs WHERE thread_id=$1 AND author_id IS NOT NULL)")
            .bind(thread).fetch_one(&mut *tx).await.map_err(orch_db::DbError::from)?;
        if present && !retained {
            orch_db::queries::hierarchy::delete_subtree(&mut tx, thread, by, now).await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    /// What follows a fork or a switch that ended, in any way: the threads it held are let go, with
    /// their queues, and what waited for its environments looks again.
    pub(crate) async fn after_fork_job(&self, row: &JobRow) {
        if let Err(error) =
            crate::fork_source::release_pins(self, row.id, self.inner.clock.now()).await
        {
            tracing::warn!(%error, job = %row.id, "a fork's exported saved-file pins await release");
        }
        if let Some(batch) = row.batch_id
            && let Err(error) = self.cleanup_batch_environment(batch).await
        {
            tracing::warn!(%error,%batch,"the unpublished environment of a finished batch could not be cleaned up");
        }
        let mut held: Vec<Id> = row.node_id.into_iter().collect();
        if let Some(new_thread) = row
            .plan
            .get("new_thread")
            .and_then(Json::as_str)
            .and_then(|text| text.parse::<Id>().ok())
        {
            held.push(new_thread);
        }
        for thread in held {
            if let Err(error) = self.release_lock(thread).await {
                tracing::warn!(%error, %thread, "a thread that a fork held could not be let go");
            }
        }
        // The person who asked is told once: for a batch, when its last job ends.
        let tell = match row.batch_id {
            None => true,
            Some(batch) => match self.batch_jobs(batch).await {
                Ok(jobs) => jobs
                    .iter()
                    .filter(|job| job.id != row.id)
                    .all(|job| !job.is_open()),
                Err(_) => false,
            },
        };
        if tell
            && let Ok(mut conn) = self.inner.db.acquire().await
            && let Err(error) =
                crate::notifications::job_finished(&mut conn, row.id, self.inner.clock.now()).await
        {
            tracing::warn!(%error, job = %row.id, "the people a job concerns could not be told that it ended");
        }
        for environment in [row.environment_id].into_iter().flatten().chain(
            row.plan
                .get("from_environment")
                .and_then(Json::as_str)
                .and_then(|text| text.parse::<Id>().ok()),
        ) {
            if let Err(error) = self.reevaluate_environment(environment).await {
                tracing::warn!(%error, "the threads of an environment could not be woken");
            }
        }
    }
}

/// How many of the last turns stand in, a few lines each, for a switch's summary that was not written: the
/// number the handoff notice has always asked a new agent to read first.
const SUMMARY_STAND_IN_TURNS: usize = 3;

/// How many bytes of recent turns a switch carries into the new agent's context, from the new model's entry.
async fn carry_budget(
    engine: &crate::engine::Engine,
    snapshot: &crate::sessions::StartSnapshot,
) -> Result<Option<usize>, Stop> {
    let window = snapshot
        .model
        .as_ref()
        .and_then(|model| fork_history::model_window(&model.parameters));
    let mut conn = engine.db().acquire().await?;
    Ok(fork_history::carry_budget(&mut conn, snapshot.thread.node_id, window).await?)
}

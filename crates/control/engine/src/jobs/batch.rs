//! Independent switches, moves and updates with bounded concurrency.
//!
//! **Several switches at once.** One provider serves many threads, so when it runs out or is retired everything
//! on it has to move; one environment may hold many threads that are all to go elsewhere. A batch is planned
//! for up to two hundred named threads, or for every thread of an environment the caller manages (the ones
//! they cannot see included, since where they run is the environment's to decide); each thread gets its own
//! plan, with its own method, report and blockers.
//!
//! Carrying a batch out makes one job for each plan without blockers, all with one `batch_id`, all locked at
//! once in one transaction, also those whose job waits its turn; at most `switch.batch_parallel` of them run
//! at a time ([`orch_db::queries::jobs::forks::claim_in_batch`]). Each job succeeds or fails on its own; a batch
//! is not all-or-nothing. A thread whose plan has blockers is left as it is. Cancelling a batch cancels its
//! queued jobs and the jobs that have not reached their commit.
//!
//! A new environment is shared by every executable member. One member makes it; the others wait in the
//! claim queue for its provisioning. Its durable maker checkpoint survives a restart. Failed members never
//! remove another member's environment; if none commits, the batch's unpublished environment is removed.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::fork_plan::{PlanError, PreparedFork, Reach, StoredBatch, StoredMember};
use crate::jobs::estimates;
use crate::jobs::runner::read_progress;
use orch_access_contract::decisions::MissingAccess as Missing;
use orch_api_contract::forks::{
    AppliedChangeOptions, ForkBatchCounts, ForkBatchPlan, ForkBatchPreflightBody, ForkBatchTarget,
    ForkCreateBody, ForkEnvironment, ForkRequest, ForkResult, ForkSkipped, SkipReason,
};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_authz::Caller;
use orch_db::DbError;
use orch_db::queries::jobs::{self as job_rows, JobRow, forks as fork_rows, state};
use orch_db::queries::threads;
use orch_types::{Id, new_id};

/// How many jobs of one batch run at once when the setting is not there: what it ships with.
const PARALLEL_DEFAULT: i64 = 4;

mod confirmation;
pub use confirmation::{BatchConfirmation, ConfirmationMember, LifecycleConfirmation};

/// A batch that is planned and not yet kept.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchDraft {
    /// The plan, with no identifier yet.
    pub plan: ForkBatchPlan,
    /// What each plan is for.
    pub members: Vec<StoredMember>,
    /// What the caller lacks, for each plan: its blockers' indexes and everything lacking.
    pub lacks: Vec<Vec<(usize, Vec<Missing>)>>,
    /// What was asked.
    pub body: ForkBatchPreflightBody,
    /// Actual native names/configuration entering the selected full plan.
    pub disclosure: Vec<orch_access_contract::decisions::ObjectRef>,
}

/// Pure native batch selection. Suppliers and private-plan insertion are separate consumers.
#[derive(Clone)]
pub struct PreparedBatch {
    body: ForkBatchPreflightBody,
    members: Vec<BatchSelection>,
    skipped: Vec<ForkSkipped>,
    new_environment: bool,
    now: chrono::DateTime<chrono::Utc>,
}
#[derive(Clone)]
pub struct BatchSelection {
    stored: StoredMember,
    selected: PreparedFork,
}
impl BatchSelection {
    pub fn selected(&self) -> &PreparedFork {
        &self.selected
    }
    pub fn member(&self) -> &StoredMember {
        &self.stored
    }
}
impl PreparedBatch {
    pub fn body(&self) -> &ForkBatchPreflightBody {
        &self.body
    }
    pub fn selections(&self) -> &[BatchSelection] {
        &self.members
    }
    pub fn preserves(&self, current: &Self) -> bool {
        self.body == current.body
            && self.skipped == current.skipped
            && self.members.len() == current.members.len()
            && self
                .members
                .iter()
                .zip(&current.members)
                .all(|(old, new)| old.stored == new.stored && old.selected.preserves(&new.selected))
    }
}

/// One actual batch membership snapshot; state may advance without changing its sources.
#[derive(Clone)]
pub struct BatchCancellation {
    batch: Id,
    rows: Vec<JobRow>,
}

impl BatchCancellation {
    /// Actual native rows, including already finished members.
    pub fn rows(&self) -> &[JobRow] {
        &self.rows
    }

    /// The same immutable native membership and governors, independently of progress.
    pub fn preserves(&self, fresh: &Self) -> bool {
        self.batch == fresh.batch
            && self.rows.len() == fresh.rows.len()
            && self.rows.iter().zip(&fresh.rows).all(|(old, new)| {
                old.id == new.id
                    && old.kind == new.kind
                    && old.node_id == new.node_id
                    && old.environment_id == new.environment_id
                    && old.provider_id == new.provider_id
                    && old.batch_id == new.batch_id
                    && old.created_by == new.created_by
                    && old.created_at == new.created_at
            })
    }
}

/// The existing native cancellation outcome; cleanup starts only after its transaction commits.
pub struct BatchMemberCancellation {
    row: JobRow,
    ended: bool,
    asked: bool,
}

impl BatchMemberCancellation {
    /// Actual affected member, never a new scope identity.
    pub fn id(&self) -> Id {
        self.row.id
    }
    /// A queued member ended in this transaction.
    pub fn ended(&self) -> bool {
        self.ended
    }
    /// A running member accepted the native cancellation request.
    pub fn asked(&self) -> bool {
        self.asked
    }
}

/// The request for one thread of a batch: a switch in place to what the batch names.
fn request_of(body: &ForkBatchPreflightBody) -> ForkRequest {
    ForkRequest {
        result: ForkResult::Replace,
        topic_id: None,
        name: None,
        access: None,
        agent_access: None,
        history: None,
        to: body.to.clone(),
        environment: body.environment.clone(),
        worktree: None,
        files_at: None,
        memory: None,
        directory: None,
        cwd: None,
        options: body.options.clone(),
        check_fingerprints: None,
        fingerprint_request_id: None,
        mounts: None,
    }
}

fn redacted(thread: Id) -> NodeRef {
    NodeRef {
        thread_display: None,
        id: thread,
        kind: NodeKind::Thread,
        name: None,
        redacted: true,
    }
}

async fn require_binding_source_read(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    object: orch_access_contract::decisions::ObjectRef,
) -> Result<(), PlanError> {
    let may_read = crate::resource_facts::authorities(conn, principal, &[object.id])
        .await?
        .into_iter()
        .any(|row| row.resource.object() == object && (row.readable || row.manageable));
    if may_read {
        Ok(())
    } else {
        Err(PlanError::NotFound)
    }
}

impl Engine {
    /// The most jobs of one batch that run at once, by the setting.
    async fn batch_parallel(&self) -> Result<i64, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        Ok(orch_settings::get::<i64>(
            &mut conn,
            orch_settings::Place::System,
            orch_settings_contract::registry::keys::SWITCH_BATCH_PARALLEL,
        )
        .await?
        .unwrap_or(PARALLEL_DEFAULT))
    }

    /// Plans a switch of several threads together, and changes nothing.
    pub async fn draft_batch(
        &self,
        caller: &Caller,
        body: &ForkBatchPreflightBody,
    ) -> Result<BatchDraft, PlanError> {
        let mut conn = self.inner.db.acquire().await?;
        let prepared = self.prepare_batch_in(&mut conn, caller, body).await?;
        self.assemble_prepared_batch(&mut conn, caller, &prepared, None)
            .await
    }

    /// Selects the actual native members and destinations without running any supplier.
    pub async fn prepare_batch_in(
        &self,
        conn: &mut orch_db::Conn,
        caller: &Caller,
        body: &ForkBatchPreflightBody,
    ) -> Result<PreparedBatch, PlanError> {
        Box::pin(
            self.prepare_batch_on::<crate::fork_plan::operation::BatchPreview>(
                conn, caller, body, false,
            ),
        )
        .await
    }
    /// The same native selection, with current candidate facts from its declared batch source.
    pub async fn prepare_batch_in_with_source<I: crate::fork_plan::operation::Intent>(
        &self,
        conn: &mut orch_db::Conn,
        caller: &Caller,
        body: &ForkBatchPreflightBody,
    ) -> Result<PreparedBatch, PlanError> {
        Box::pin(self.prepare_batch_on::<I>(conn, caller, body, true)).await
    }
    async fn prepare_batch_on<I: crate::fork_plan::operation::Intent>(
        &self,
        conn: &mut orch_db::Conn,
        caller: &Caller,
        body: &ForkBatchPreflightBody,
        scoped_preparation: bool,
    ) -> Result<PreparedBatch, PlanError> {
        let now = self.inner.clock.now();
        let most = orch_settings::get::<i64>(
            conn,
            orch_settings::Place::System,
            orch_settings_contract::registry::keys::SWITCH_BATCH_THREADS,
        )
        .await
        .map_err(EngineError::from)?
        .ok_or_else(|| EngineError::unreadable("the batch thread setting is missing"))?;
        let most = usize::try_from(most)
            .map_err(|_| EngineError::unreadable("the batch thread setting is invalid"))?;
        if body.source_model_id.is_some() && body.all_of_provider.is_none() {
            return Err(PlanError::invalid(
                "source_model_id",
                "goes with all_of_provider",
            ));
        }
        let mut excluded = Vec::new();
        let (named, whole_environment) = match (
            &body.thread_ids,
            body.all_of_environment,
            body.all_of_provider,
            body.all_of_agent,
        ) {
            (Some(ids), None, None, None) => {
                if ids.is_empty() {
                    return Err(PlanError::invalid("thread_ids", "names a thread"));
                }
                if ids.len() > most {
                    return Err(PlanError::invalid(
                        "thread_ids",
                        format!("names at most {most} threads, as set by switch.batch_threads"),
                    ));
                }
                (ids.clone(), None)
            }
            (None, Some(environment), None, None) => {
                if body.to.as_ref().is_some_and(|to| {
                    to.provider_id.is_some()
                        || to.agent_id.is_some()
                        || to.model.is_some()
                        || to.effort.is_some()
                        || to.arrangement.is_some()
                }) {
                    return Err(PlanError::invalid(
                        "to",
                        "all the threads of an environment change their environment alone",
                    ));
                }
                // The environment's manager, and nobody else, names all its threads.
                let answer = self
                    .inner
                    .access
                    .environment_manage(conn, caller.user(), environment)
                    .await
                    .map_err(EngineError::from)?;
                if !answer.is_allowed() {
                    return Err(PlanError::NotFound);
                }
                let ids = threads::tools::threads_in_environment(conn, environment).await?;
                if ids.len() > most {
                    return Err(PlanError::invalid(
                        "all_of_environment",
                        format!(
                            "the environment holds more than {most} threads, as set by switch.batch_threads"
                        ),
                    ));
                }
                (ids, Some(environment))
            }
            (None, None, Some(source), None) | (None, None, None, Some(source)) => {
                use orch_access_contract::decisions::ObjectRef;
                use orch_access_contract::permissions::{ObjectKind, Permission, PermissionName};
                use orch_api_contract::resource_facts::{DependentRelation, FactTarget};
                let object = ObjectRef {
                    kind: if body.all_of_agent.is_some() {
                        ObjectKind::Agent
                    } else {
                        ObjectKind::Provider
                    },
                    id: source,
                };
                let ordinary = if scoped_preparation
                    && caller
                        .grant()
                        .is_some_and(|grant| grant.operation_scope().is_some())
                {
                    None
                } else {
                    let principal = orch_authz::Principal::load(conn, caller.clone(), now.into())
                        .await
                        .map_err(EngineError::from)?;
                    require_binding_source_read(conn, &principal, object).await?;
                    Some(principal)
                };
                if let Some(model) = body.source_model_id {
                    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM provider_models WHERE id=$1 AND provider_id=$2)")
                        .bind(model).bind(source).fetch_one(&mut *conn).await.map_err(DbError::from)?;
                    if !exists {
                        return Err(PlanError::NotFound);
                    }
                }
                let facts = crate::resource_facts::target_dependents(
                    conn,
                    &FactTarget {
                        object,
                        model_id: body.source_model_id,
                    },
                    now.into(),
                )
                .await?;
                let ids: Vec<_> = facts
                    .dependents
                    .iter()
                    .filter(|edge| edge.relation == DependentRelation::Binding)
                    .filter_map(|edge| {
                        edge.object
                            .filter(|object| object.kind == ObjectKind::Thread)
                            .map(|object| object.id)
                    })
                    .collect();
                let principal = match ordinary {
                    Some(principal) => principal,
                    None => {
                        let candidate = crate::fork_plan::operation::batch_discovery_caller::<I>(
                            conn, caller, &ids,
                        )
                        .await?;
                        let principal = orch_authz::Principal::load(conn, candidate, now.into())
                            .await
                            .map_err(EngineError::from)?;
                        require_binding_source_read(conn, &principal, object).await?;
                        principal
                    }
                };
                let managed = orch_authz::nodes::holding(
                    conn,
                    &principal,
                    &ids,
                    PermissionName::Exact(Permission::ThreadManage),
                    orch_authz::Act::Use,
                    orch_authz::Policy::default(),
                )
                .await
                .map_err(EngineError::from)?;
                let managed: std::collections::HashSet<_> =
                    managed.into_iter().map(|row| row.id).collect();
                excluded = ids
                    .iter()
                    .filter(|id| !managed.contains(id))
                    .copied()
                    .collect();
                let named: Vec<_> = ids.into_iter().filter(|id| managed.contains(id)).collect();
                if named.len() > most {
                    return Err(PlanError::invalid(
                        if body.all_of_agent.is_some() {
                            "all_of_agent"
                        } else {
                            "all_of_provider"
                        },
                        format!("names more than {most} threads, as set by switch.batch_threads"),
                    ));
                }
                (named, None)
            }
            _ => {
                return Err(PlanError::invalid(
                    "thread_ids",
                    "exactly one of thread_ids, all_of_environment, all_of_provider and all_of_agent is given",
                ));
            }
        };
        if whole_environment.is_some() && body.thread_ids.is_some() {
            return Err(PlanError::invalid(
                "thread_ids",
                "not with all_of_environment",
            ));
        }
        let mut request = request_of(body);
        let new_environment = matches!(request.environment, Some(ForkEnvironment::New { .. }));
        let reach = if whole_environment.is_some() {
            Reach::Environment
        } else {
            Reach::Seen
        };
        let mut members = Vec::new();
        let mut skipped: Vec<ForkSkipped> = excluded
            .into_iter()
            .map(|thread| ForkSkipped {
                thread: redacted(thread),
                reason: SkipReason::NoPermission,
            })
            .collect();
        let mut seen = std::collections::BTreeSet::new();
        for thread in named {
            if !seen.insert(thread) {
                continue;
            }
            let skip = |reason| ForkSkipped {
                thread: redacted(thread),
                reason,
            };
            // The actual native source is known before visibility and permission selection.
            // Missing/deleted native rows keep their original skip and never invent a target.
            let candidate_caller = if scoped_preparation {
                if threads::thread(conn, thread)
                    .await?
                    .is_none_or(|row| row.deleted_at.is_some())
                {
                    skipped.push(skip(SkipReason::NotFound));
                    continue;
                }
                crate::fork_plan::operation::batch_preparation_caller::<I>(
                    conn,
                    caller,
                    whole_environment,
                    &[thread],
                )
                .await?
            } else {
                caller.clone()
            };
            match self
                .prepare_fork_in(conn, &candidate_caller, thread, &request, reach, None, None)
                .await
            {
                Ok(selected) => {
                    if selected.missing_thread_manage() {
                        let mut left = skip(SkipReason::NoPermission);
                        left.thread = if reach == Reach::Environment
                            && !self
                                .caller_sees(conn, &candidate_caller, thread, now)
                                .await?
                        {
                            redacted(thread)
                        } else {
                            selected.source_ref()
                        };
                        skipped.push(left);
                        continue;
                    }
                    selected
                        .share_environment(conn, &mut request.environment)
                        .await?;
                    members.push(BatchSelection {
                        stored: StoredMember {
                            thread,
                            request: request.clone(),
                            reach,
                            source_scope: Some(crate::fork_plan::SourceScope::of(
                                selected.source(),
                                reach,
                            )),
                        },
                        selected,
                    });
                }
                Err(PlanError::NotFound) => skipped.push(skip(SkipReason::NotFound)),
                Err(PlanError::Archived) => skipped.push(skip(SkipReason::Archived)),
                Err(PlanError::Side) => skipped.push(skip(SkipReason::NoPermission)),
                Err(other) => return Err(other),
            }
        }
        Ok(PreparedBatch {
            body: body.clone(),
            members,
            skipped,
            new_environment,
            now,
        })
    }

    /// The existing native supplier half, consuming the exact selected members once.
    pub async fn assemble_prepared_batch(
        &self,
        conn: &mut orch_db::Conn,
        caller: &Caller,
        prepared: &PreparedBatch,
        sources: Option<&[crate::fork_plan::operation::Preparation]>,
    ) -> Result<BatchDraft, PlanError> {
        if sources.is_some_and(|sources| sources.len() != prepared.members.len()) {
            return Err(
                EngineError::unreadable("the native batch supplier source count changed").into(),
            );
        }
        let mut plans = Vec::new();
        let mut members = Vec::new();
        let mut lacks = Vec::new();
        let skipped = prepared.skipped.clone();
        let mut disclosure = Vec::new();
        for (index, member) in prepared.members.iter().enumerate() {
            let mut draft = self
                .assemble_prepared_fork_with_source(
                    conn,
                    &member.selected,
                    sources.map(|sources| &sources[index]),
                )
                .await?;
            if member.stored.reach == Reach::Environment
                && !self
                    .caller_sees(conn, caller, member.stored.thread, prepared.now)
                    .await?
            {
                draft.plan.source = redacted(member.stored.thread);
            }
            lacks.push(draft.lacks);
            members.push(member.stored.clone());
            disclosure.extend(draft.disclosure);
            plans.push(draft.plan);
        }
        let parallel = self.batch_parallel().await?;
        let executable = plans.iter().filter(|plan| plan.blockers.is_empty()).count();
        let first = plans.first();
        let options = first.map_or_else(
            || default_options(&prepared.body),
            |plan| plan.options.clone(),
        );
        let estimate = (if prepared.new_environment {
            estimates::together_in_new_environment
        } else {
            estimates::together
        })(
            &plans
                .iter()
                .map(|plan| plan.estimate.clone())
                .collect::<Vec<_>>(),
            parallel,
        );
        let target = ForkBatchTarget {
            provider: prepared
                .body
                .to
                .as_ref()
                .and_then(|to| to.provider_id)
                .and(first.map(|plan| plan.to.provider.clone())),
            agent: prepared
                .body
                .to
                .as_ref()
                .and_then(|to| to.agent_id)
                .and(first.map(|plan| plan.to.agent.clone())),
            model: prepared.body.to.as_ref().and_then(|to| to.model.clone()),
            effort: prepared.body.to.as_ref().and_then(|to| to.effort.clone()),
        };
        let plan = ForkBatchPlan {
            notification: None,
            preflight_id: Id::nil(),
            expires_at: first.map_or_else(
                || {
                    orch_types::Time::new(
                        prepared.now
                            + chrono::Duration::minutes(
                                orch_api_contract::forks::PREFLIGHT_VALID_MINUTES,
                            ),
                    )
                },
                |plan| plan.expires_at,
            ),
            to: target,
            turns_running: i64::try_from(plans.iter().filter(|plan| plan.turn_running).count())
                .unwrap_or(i64::MAX),
            executable: i64::try_from(executable).unwrap_or(i64::MAX),
            options,
            estimate,
            skipped,
            plans,
        };
        Ok(BatchDraft {
            plan,
            members,
            lacks,
            body: prepared.body.clone(),
            disclosure,
        })
    }

    /// Whether the caller may see a thread.
    async fn caller_sees(
        &self,
        conn: &mut orch_db::Conn,
        caller: &Caller,
        thread: Id,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, PlanError> {
        let principal =
            orch_authz::Principal::load(&mut *conn, caller.clone(), orch_types::Time::new(now))
                .await?;
        Ok(matches!(
            orch_authz::nodes::check(
                conn,
                &principal,
                thread,
                orch_access_contract::permissions::Permission::ThreadView,
                orch_authz::Act::Use,
                orch_authz::Policy::default(),
            )
            .await,
            Ok(orch_access_contract::decisions::Decision::Allowed { .. })
                | Err(orch_authz::Error::Archived { .. })
        ))
    }

    /// Keeps the plan of a batch for the minutes it is good for, and returns it with its identifier.
    pub async fn keep_batch_plan(
        &self,
        caller: &Caller,
        draft: BatchDraft,
    ) -> Result<ForkBatchPlan, PlanError> {
        let mut conn = self.inner.db.acquire().await?;
        self.keep_batch_plan_on(&mut conn, caller, draft, || Ok(()))
            .await
    }
    /// The same audience/private-plan writer at its first INSERT on the actual action connection.
    pub async fn keep_batch_plan_on(
        &self,
        conn: &mut orch_db::Conn,
        caller: &Caller,
        mut draft: BatchDraft,
        before: impl FnOnce() -> Result<(), EngineError> + Send,
    ) -> Result<ForkBatchPlan, PlanError> {
        let id = new_id();
        draft.plan.preflight_id = id;
        let principal = orch_authz::Principal::load(
            conn,
            caller.clone(),
            orch_types::Time::new(self.inner.clock.now()),
        )
        .await?;
        let mut stored = StoredBatch {
            body: draft.body.clone(),
            plan: draft.plan.clone(),
            members: draft.members.clone(),
        };
        let ids = stored.recipient_threads();
        let audience =
            crate::notifications::operation_thread_audience(conn, &principal, &ids).await?;
        draft.plan.notification = Some(orch_api_contract::loss::OperationNotificationPlan {
            preview_id: Some(id),
            people: audience.len() as i64,
            default_on: draft.plan.executable > 0,
        });
        stored.plan = draft.plan.clone();
        before()?;
        fork_rows::save_preflight(
            conn,
            &fork_rows::PreflightRow {
                id,
                kind: fork_rows::BATCH_PLAN_KIND.to_owned(),
                node_id: None,
                environment_id: draft.body.all_of_environment,
                created_by: caller.user(),
                plan: serde_json::to_value(&stored)?,
                expires_at: draft.plan.expires_at.get(),
                created_at: self.inner.clock.now(),
            },
        )
        .await?;
        orch_db::queries::notifications::operations::preview(
            conn,
            id,
            &audience
                .iter()
                .map(|person| person.user)
                .collect::<Vec<_>>(),
        )
        .await?;
        Ok(draft.plan)
    }

    /// Carries out the plan of a batch: one job for each plan without blockers, all of them locked in this
    /// transaction, those that wait their turn too. Plans with blockers are left alone; a batch with none
    /// to carry out is refused. The answer is the batch and its jobs.
    pub async fn confirm_batch(
        &self,
        caller: &Caller,
        body: &ForkCreateBody,
    ) -> Result<(Id, Vec<JobRow>), PlanError> {
        let mut tx = self.inner.db.begin().await?;
        let row = fork_rows::lock_preflight(&mut tx, body.preflight_id)
            .await?
            .filter(|row| row.kind == fork_rows::BATCH_PLAN_KIND)
            .filter(|row| row.created_by == caller.user())
            .ok_or(PlanError::NoSuchPlan)?;
        if row.expires_at <= self.inner.clock.now() {
            return Err(PlanError::Expired);
        }
        let stored: StoredBatch = serde_json::from_value(row.plan)?;
        let batch = new_id();
        let principal = orch_authz::Principal::load(
            &mut tx,
            caller.clone(),
            orch_types::Time::new(self.inner.clock.now()),
        )
        .await?;
        let jobs = crate::notifications::operation_scope(
            Some(batch),
            self.queue_batch_members(&mut tx, caller, &stored, body, batch),
        )
        .await?;
        Box::pin(self.finish_batch_confirmation(tx, &principal, body, batch, jobs, || Ok(()))).await
    }
    pub(super) async fn finish_batch_confirmation(
        &self,
        mut tx: orch_db::Tx,
        principal: &orch_authz::Principal,
        body: &ForkCreateBody,
        batch: Id,
        jobs: Vec<JobRow>,
        deadline: impl Fn() -> Result<(), EngineError> + Send,
    ) -> Result<(Id, Vec<JobRow>), PlanError> {
        let actual = jobs
            .iter()
            .filter_map(|job| job.node_id)
            .collect::<Vec<_>>();
        let audience =
            crate::notifications::operation_thread_audience(&mut tx, principal, &actual).await?;
        let reviewed: Option<Vec<Id>> =
            sqlx::query_scalar("SELECT users FROM operation_recipient_previews WHERE id=$1")
                .bind(body.preflight_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(orch_db::DbError::from)?;
        if body.notify == Some(true)
            && reviewed.is_some_and(|reviewed| {
                reviewed
                    != audience
                        .iter()
                        .map(|person| person.user)
                        .collect::<Vec<_>>()
            })
        {
            return Err(PlanError::Invalid {
                path: "notify".into(),
                message: "The people affected changed; review the batch again".into(),
            });
        }
        let operation = crate::notifications::NotificationOperation {
            id: batch,
            kind: "switch".into(),
            name: String::new(),
            notify: body.notify.unwrap_or(false),
            path: "/".into(),
            subject: None,
            recipients: audience,
        };
        crate::notifications::attach_operation(
            &mut tx,
            &jobs.iter().map(|job| job.id).collect::<Vec<_>>(),
            &operation,
            self.inner.clock.now(),
        )
        .await?;
        crate::notifications::notify_operation(
            &mut tx,
            &operation,
            "running",
            self.inner.clock.now(),
        )
        .await?;
        fork_rows::forget_preflight(&mut tx, body.preflight_id).await?;
        deadline()?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.kick();
        for job in &jobs {
            if let Some(thread) = job.node_id {
                self.wake(thread);
            }
        }
        Ok((batch, jobs))
    }

    /// Queues independently reviewed switch members in a caller's transaction and shared runtime batch.
    pub async fn queue_batch_members(
        &self,
        conn: &mut orch_db::Tx,
        caller: &Caller,
        stored: &StoredBatch,
        body: &ForkCreateBody,
        batch: Id,
    ) -> Result<Vec<JobRow>, PlanError> {
        if !stored.members.is_empty()
            && stored
                .members
                .iter()
                .all(|member| member.reach == Reach::Environment)
        {
            let selected = self
                .select_lifecycle_confirmation(conn, caller, stored, body, batch, true)
                .await?;
            let drafts = self
                .assemble_lifecycle_confirmation(conn, &selected)
                .await?;
            return self
                .queue_lifecycle_confirmation(conn, caller, &selected, &selected, &drafts)
                .await;
        }
        let prepared = Box::pin(self.select_confirmation_members(
            conn,
            caller,
            stored,
            body,
            batch,
            confirmation::Selection {
                locking: true,
                declared: false,
            },
        ))
        .await?;
        let drafts = Box::pin(self.assemble_confirmation_members(conn, &prepared, None)).await?;
        Box::pin(self.queue_confirmation_members(
            conn,
            caller,
            &prepared,
            drafts,
            None,
            confirmation::Queue {
                batch,
                preflight: body.preflight_id,
                new_environment: matches!(
                    stored.body.environment,
                    Some(ForkEnvironment::New { .. })
                ),
            },
            || Ok(()),
            || Ok(()),
        ))
        .await
    }

    /// The jobs of a batch, oldest first.
    pub async fn batch_jobs(&self, batch: Id) -> Result<Vec<JobRow>, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        Ok(fork_rows::of_batch(&mut conn, batch).await?)
    }

    /// Remove an unpublished batch environment only after every member has ended without committing.
    pub(crate) async fn cleanup_batch_environment(&self, batch: Id) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text,0))")
            .bind(batch)
            .execute(&mut *tx)
            .await
            .map_err(orch_db::DbError::from)?;
        let jobs = fork_rows::of_batch(&mut tx, batch).await?;
        if jobs.iter().any(|job| {
            job.is_open() || job.state == state::SUCCEEDED || job.plan.get("committed").is_some()
        }) {
            return Ok(());
        }
        let maker = jobs.iter().find_map(|job| {
            job.plan
                .get("batch_environment_job")
                .and_then(serde_json::Value::as_str)
                .and_then(|id| id.parse::<Id>().ok())
        });
        let Some(maker) = maker.and_then(|id| jobs.iter().find(|job| job.id == id)) else {
            return Ok(());
        };
        let Some(environment) = maker
            .plan
            .get("new_environment")
            .and_then(serde_json::Value::as_str)
            .and_then(|id| id.parse::<Id>().ok())
        else {
            return Ok(());
        };
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        // A later ordinary binding also keeps it: a batch never removes another person's accepted work.
        let used: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM threads WHERE environment_id=$1)")
                .bind(environment)
                .fetch_one(&mut *tx)
                .await
                .map_err(orch_db::DbError::from)?;
        if used {
            return Ok(());
        }
        let now = self.inner.clock.now();
        job_rows::merge_plan(
            &mut tx,
            maker.id,
            &serde_json::json!({"scratch_cleanup":environment}),
            now,
        )
        .await?;
        orch_db::queries::environments::hosting::schedule_removal(&mut tx, environment, now, now)
            .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.kick();
        Ok(())
    }

    /// Other open/committed members may use a shared directory. Undo never removes their files.
    pub(crate) async fn batch_keeps_directory(
        &self,
        batch: Id,
        job: Id,
        environment: Id,
        directory: &str,
    ) -> Result<bool, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let env =
            orch_db::queries::environments::thread_engine::environment(&mut conn, environment)
                .await?;
        let Some(env) = env else {
            return Ok(false);
        };
        let rows = fork_rows::of_batch(&mut conn, batch).await?;
        let overlaps = |a: &str, b: &str| {
            let a = a.trim_end_matches('/');
            let b = b.trim_end_matches('/');
            a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
        };
        Ok(rows
            .iter()
            .filter(|row| row.id != job && (row.is_open() || row.plan.get("committed").is_some()))
            .any(|row| {
                let copied = row
                    .plan
                    .pointer("/copied/directory")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                let planned = row
                    .plan
                    .pointer("/forkplan/to/directory")
                    .and_then(serde_json::Value::as_str)
                    .map(|path| crate::sessions::absolute_directory(&env.workspace_root, path));
                copied
                    .or(planned)
                    .is_some_and(|path| overlaps(directory, &path))
            }))
    }

    /// Cancels the jobs of a batch that have not passed their commit: the queued ones at once, the ones
    /// that run as a request to cancel. Returns the jobs as they stand.
    pub async fn cancel_batch(&self, by: Id, batch: Id) -> Result<Vec<JobRow>, EngineError> {
        let rows = self.batch_jobs(batch).await?;
        for row in rows.iter().filter(|row| row.is_open()) {
            let mut tx = self.inner.db.begin().await?;
            let Some(current) = job_rows::lock(&mut tx, row.id).await? else {
                continue;
            };
            let result = self
                .cancel_batch_member_before(&mut tx, &current, by, || Ok(()))
                .await?;
            tx.commit().await.map_err(DbError::from)?;
            self.finish_batch_member(&result).await?;
        }
        self.batch_jobs(batch).await
    }

    /// Select the exact native batch without dispatching or cancelling anything.
    pub async fn preparing_batch_cancellation(
        &self,
        conn: &mut orch_db::Conn,
        batch: Id,
    ) -> Result<BatchCancellation, EngineError> {
        Ok(BatchCancellation {
            batch,
            rows: fork_rows::of_batch(conn, batch).await?,
        })
    }

    /// The same per-member writer used by ordinary cancellation, on the caller's locked row.
    /// The callback belongs immediately before the first actual SQL mutation.
    pub async fn cancel_batch_member_before<F>(
        &self,
        conn: &mut orch_db::Conn,
        row: &JobRow,
        by: Id,
        before: F,
    ) -> Result<BatchMemberCancellation, EngineError>
    where
        F: FnOnce() -> Result<(), EngineError> + Send,
    {
        let mut result = BatchMemberCancellation {
            row: row.clone(),
            ended: false,
            asked: false,
        };
        if !row.is_open() {
            return Ok(result);
        }
        let never_began = row.state == state::QUEUED && row.plan.get("runner").is_none();
        if never_began {
            let (progress, _) = read_progress(row);
            let progress_json = serde_json::to_value(&progress)?;
            let error = serde_json::json!({"code":"cancelled", "message":"the job was cancelled before it began", "phase":progress.phase, "undone":true});
            before()?;
            result.ended = job_rows::finish(
                conn,
                row.id,
                state::CANCELLED,
                &progress_json,
                None,
                Some(&error),
                self.inner.clock.now(),
            )
            .await?;
        } else {
            before()?;
            result.asked =
                job_rows::request_cancel(conn, row.id, by, self.inner.clock.now()).await?;
        }
        Ok(result)
    }

    /// Complete only the already accepted cancellation's native cleanup and wakeups.
    pub async fn finish_batch_member(
        &self,
        result: &BatchMemberCancellation,
    ) -> Result<(), EngineError> {
        if result.ended {
            self.after_fork_job(&result.row).await;
        }
        if result.asked {
            self.stop_job_command(result.row.id).await?;
            self.kick();
        }
        Ok(())
    }
}

/// The options a batch with no plan shows: what the settings ship with, the request's laid over them.
fn default_options(body: &ForkBatchPreflightBody) -> AppliedChangeOptions {
    let given = body.options.clone().unwrap_or_default();
    AppliedChangeOptions {
        active_turn: given
            .active_turn
            .unwrap_or(orch_api_contract::forks::ActiveTurn::Wait),
        wait_minutes: given.wait_minutes.unwrap_or(10),
        verify_timeout_secs: given.verify_timeout_secs.unwrap_or(120),
        keep_previous: given.keep_previous.unwrap_or(false),
        stop_processes: given.stop_processes.unwrap_or(true),
    }
}

/// How many jobs of a batch are in each state.
#[must_use]
pub fn counts_of(rows: &[JobRow]) -> ForkBatchCounts {
    let count = |word: &str| {
        i64::try_from(rows.iter().filter(|row| row.state == word).count()).unwrap_or(i64::MAX)
    };
    ForkBatchCounts {
        queued: count(state::QUEUED),
        running: count(state::RUNNING),
        succeeded: count(state::SUCCEEDED),
        failed: count(state::FAILED),
        cancelled: count(state::CANCELLED),
    }
}

/// How far a batch has come: the mean of the percentages of its jobs, a job that succeeded being whole.
#[must_use]
pub fn percent_of_batch(rows: &[JobRow]) -> u32 {
    if rows.is_empty() {
        return 0;
    }
    let sum: u32 = rows
        .iter()
        .map(|row| {
            if row.state == state::SUCCEEDED {
                100
            } else {
                read_progress(row).0.percent
            }
        })
        .sum();
    sum / u32::try_from(rows.len()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn row(state: &str, percent: u32) -> JobRow {
        JobRow {
            id: new_id(),
            kind: "thread_fork".to_owned(),
            node_id: None,
            environment_id: None,
            provider_id: None,
            batch_id: None,
            state: state.to_owned(),
            plan: serde_json::json!({}),
            progress: serde_json::json!({
                "phase": "copy", "phases": [], "percent": percent, "files_done": 0, "files_total": null,
                "bytes_done": 0, "bytes_total": null, "rate_bps": null, "eta_seconds": null, "message": null
            }),
            result: None,
            error: None,
            created_by: None,
            topic_seq: None,
            env_seq: None,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn a_batch_is_as_far_as_the_mean_of_its_jobs() {
        let rows = [
            row("succeeded", 100),
            row("running", 40),
            row("queued", 0),
            row("failed", 20),
        ];
        // The one that waits counts for nothing, and every job counts for a quarter.
        assert_eq!(percent_of_batch(&rows), (100 + 40 + 20) / 4);
        let counts = counts_of(&rows);
        assert_eq!(
            (
                counts.queued,
                counts.running,
                counts.succeeded,
                counts.failed,
                counts.cancelled
            ),
            (1, 1, 1, 1, 0)
        );
        assert_eq!(percent_of_batch(&[]), 0);
    }
}

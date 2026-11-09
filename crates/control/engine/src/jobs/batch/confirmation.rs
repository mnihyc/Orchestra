//! Pure kept-batch confirmation selection and once-only native member assembly.
use super::*;
use crate::fork_plan::{
    Draft,
    operation::{self as source, BatchConfirm},
};

/// Keep every candidate in native order: duplicate threads can have different stored
/// operands, and the same old member may justify more than one fresh occurrence.
fn indexed_positions<T>(
    old: &[T],
    fresh: &[T],
    thread: impl Fn(&T) -> Id,
    preserves: impl Fn(&T, &T) -> bool,
) -> Option<Vec<usize>> {
    if fresh.is_empty() {
        return Some(Vec::new());
    }
    let mut by_thread = std::collections::BTreeMap::<Id, Vec<usize>>::new();
    for (position, member) in old.iter().enumerate() {
        by_thread.entry(thread(member)).or_default().push(position);
    }
    fresh
        .iter()
        .map(|member| {
            by_thread
                .get(&thread(member))?
                .iter()
                .copied()
                .find(|position| preserves(&old[*position], member))
        })
        .collect()
}

fn member_positions(
    old: &[ConfirmationMember],
    fresh: &[ConfirmationMember],
) -> Option<Vec<usize>> {
    indexed_positions(
        old,
        fresh,
        |member| member.stored.thread,
        |old, fresh| old.stored == fresh.stored && old.selected.preserves(&fresh.selected),
    )
}

#[derive(Clone)]
pub struct BatchConfirmation {
    preflight: fork_rows::PreflightRow,
    stored: StoredBatch,
    body: ForkCreateBody,
    batch: Id,
    members: Vec<ConfirmationMember>,
    actor: Id,
    transaction: i64,
}
#[derive(Clone)]
pub struct ConfirmationMember {
    stored: StoredMember,
    thread: threads::ThreadRow,
    selected: PreparedFork,
    actor: Id,
    transaction: i64,
}
impl ConfirmationMember {
    pub fn member(&self) -> &StoredMember {
        &self.stored
    }
    pub fn selected(&self) -> &PreparedFork {
        &self.selected
    }
    pub fn source(&self) -> &threads::ThreadRow {
        &self.thread
    }
}
impl BatchConfirmation {
    pub fn batch(&self) -> Id {
        self.batch
    }
    pub fn preflight_id(&self) -> Id {
        self.preflight.id
    }
    pub fn body(&self) -> &ForkCreateBody {
        &self.body
    }
    pub fn stored(&self) -> &StoredBatch {
        &self.stored
    }
    pub fn selections(&self) -> &[ConfirmationMember] {
        &self.members
    }
    /// Current native skips may remove a member; they cannot substitute another measured source.
    pub fn preserves(&self, current: &Self) -> bool {
        self.actor == current.actor
            && self.batch == current.batch
            && self.preflight == current.preflight
            && self.stored == current.stored
            && self.body == current.body
            && member_positions(&self.members, &current.members).is_some()
    }
}
/// An enclosing environment lifecycle act owns the saved batch and the complete source proof.
/// This native value is neither a fork operation alias nor a deserializable permission.
#[derive(Clone)]
pub struct LifecycleConfirmation {
    stored: StoredBatch,
    body: ForkCreateBody,
    batch: Id,
    actor: Id,
    members: Vec<ConfirmationMember>,
}
impl LifecycleConfirmation {
    pub fn preserves(&self, current: &Self) -> bool {
        self.positions(current).is_some()
    }

    fn positions(&self, current: &Self) -> Option<Vec<usize>> {
        if self.actor != current.actor
            || self.batch != current.batch
            || self.stored != current.stored
            || self.body != current.body
        {
            return None;
        }
        member_positions(&self.members, &current.members)
    }
}

fn requested(member: &StoredMember, body: &ForkCreateBody) -> ForkRequest {
    let mut request = member.request.clone();
    if let Some(given) = &body.options {
        let mut options = request.options.take().unwrap_or_default();
        options.active_turn = given.active_turn.or(options.active_turn);
        options.wait_minutes = given.wait_minutes.or(options.wait_minutes);
        options.verify_timeout_secs = given.verify_timeout_secs.or(options.verify_timeout_secs);
        options.keep_previous = given.keep_previous.or(options.keep_previous);
        options.stop_processes = given.stop_processes.or(options.stop_processes);
        request.options = Some(options);
    }
    request
}
impl Engine {
    /// Select the same native environment-reach members under an enclosing lifecycle source.
    pub async fn select_lifecycle_confirmation(
        &self,
        conn: &mut orch_db::Conn,
        caller: &Caller,
        stored: &StoredBatch,
        body: &ForkCreateBody,
        batch: Id,
        locking: bool,
    ) -> Result<LifecycleConfirmation, PlanError> {
        if stored
            .members
            .iter()
            .any(|member| member.reach != Reach::Environment)
        {
            return Err(EngineError::unreadable(
                "an environment lifecycle selected another native reach",
            )
            .into());
        }
        let members = self
            .select_confirmation_members(
                conn,
                caller,
                stored,
                body,
                batch,
                Selection {
                    locking,
                    declared: false,
                },
            )
            .await?;
        Ok(LifecycleConfirmation {
            stored: stored.clone(),
            body: body.clone(),
            batch,
            actor: caller.user(),
            members,
        })
    }
    /// The existing native suppliers run once, before repeatable lifecycle effect preparation.
    pub async fn assemble_lifecycle_confirmation(
        &self,
        conn: &mut orch_db::Conn,
        selected: &LifecycleConfirmation,
    ) -> Result<Vec<Option<Draft>>, PlanError> {
        self.assemble_confirmation_members(conn, &selected.members, None)
            .await
    }
    /// Queue only the original measured members that remain selected in this transaction.
    pub async fn queue_lifecycle_confirmation(
        &self,
        tx: &mut orch_db::Tx,
        caller: &Caller,
        original: &LifecycleConfirmation,
        current: &LifecycleConfirmation,
        drafts: &[Option<Draft>],
    ) -> Result<Vec<JobRow>, PlanError> {
        if caller.user() != current.actor || drafts.len() != original.members.len() {
            return Err(PlanError::Expired);
        }
        let positions = original.positions(current).ok_or(PlanError::Expired)?;
        let selected = positions
            .into_iter()
            .map(|position| drafts[position].clone())
            .collect();
        self.queue_confirmation_members(
            tx,
            caller,
            &current.members,
            selected,
            None,
            Queue {
                batch: current.batch,
                preflight: current.body.preflight_id,
                new_environment: matches!(
                    current.stored.body.environment,
                    Some(ForkEnvironment::New { .. })
                ),
            },
            || Ok(()),
            || Ok(()),
        )
        .await
    }

    /// Reads the actual private plan and selects its native members without locks, effects or supplier IO.
    pub async fn select_batch_confirmation(
        &self,
        conn: &mut orch_db::Conn,
        caller: &Caller,
        body: &ForkCreateBody,
    ) -> Result<BatchConfirmation, PlanError> {
        Box::pin(self.batch_confirmation_on(conn, caller, body, new_id(), false)).await
    }
    /// Repeats that same selection under its original sorted source-row footprint.
    pub async fn prepare_batch_confirmation(
        &self,
        conn: &mut orch_db::Conn,
        caller: &Caller,
        body: &ForkCreateBody,
        batch: Id,
    ) -> Result<BatchConfirmation, PlanError> {
        Box::pin(self.batch_confirmation_on(conn, caller, body, batch, true)).await
    }
    async fn batch_confirmation_on(
        &self,
        conn: &mut orch_db::Conn,
        caller: &Caller,
        body: &ForkCreateBody,
        batch: Id,
        locking: bool,
    ) -> Result<BatchConfirmation, PlanError> {
        let row = if locking {
            fork_rows::lock_preflight(conn, body.preflight_id).await?
        } else {
            fork_rows::preflight(conn, body.preflight_id).await?
        }
        .filter(|row| row.kind == fork_rows::BATCH_PLAN_KIND && row.created_by == caller.user())
        .ok_or(PlanError::NoSuchPlan)?;
        if row.expires_at <= self.inner.clock.now() {
            return Err(PlanError::Expired);
        }
        let stored: StoredBatch = serde_json::from_value(row.plan.clone())?;
        let members = self
            .select_confirmation_members(
                conn,
                caller,
                &stored,
                body,
                batch,
                Selection {
                    locking,
                    declared: true,
                },
            )
            .await?;
        let transaction = if locking {
            sqlx::query_scalar("SELECT txid_current()::bigint")
                .fetch_one(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?
        } else {
            0
        };
        Ok(BatchConfirmation {
            preflight: row,
            stored,
            body: body.clone(),
            batch,
            members,
            actor: caller.user(),
            transaction,
        })
    }
    /// Original native queue selection, shared by ordinary callers and declared confirmation.
    pub(super) async fn select_confirmation_members(
        &self,
        conn: &mut orch_db::Conn,
        caller: &Caller,
        stored: &StoredBatch,
        body: &ForkCreateBody,
        batch: Id,
        mode: Selection,
    ) -> Result<Vec<ConfirmationMember>, PlanError> {
        let transaction = if mode.locking {
            sqlx::query_scalar("SELECT txid_current()::bigint")
                .fetch_one(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?
        } else {
            0
        };
        let mut ordered: Vec<_> = stored.members.iter().collect();
        ordered.sort_by_key(|member| member.thread);
        let mut selected = Vec::new();
        let mut seen = std::collections::BTreeMap::new();
        for member in ordered {
            if let Some(old) = seen.insert(member.thread, member) {
                if old != member {
                    return Err(PlanError::Expired);
                }
                continue;
            }
            let thread = if mode.locking {
                threads::lock(conn, member.thread).await?
            } else {
                threads::thread(conn, member.thread).await?
            };
            let Some(thread) = thread.filter(|row| row.deleted_at.is_none()) else {
                continue;
            };
            if !member
                .source_scope
                .as_ref()
                .is_some_and(|scope| scope.matches(&thread))
                || crate::locks::held_by(conn, member.thread).await?.is_some()
            {
                continue;
            }
            let caller = if mode.declared {
                let environment = match member.reach {
                    Reach::Environment => match thread.environment_id {
                        Some(id) => Some(id),
                        None => continue,
                    },
                    Reach::Seen => None,
                };
                source::batch_preparation_caller::<BatchConfirm>(
                    conn,
                    caller,
                    environment,
                    &[member.thread],
                )
                .await?
            } else {
                caller.clone()
            };
            let request = requested(member, body);
            let Ok(prepared) = self
                .prepare_fork_in(
                    conn,
                    &caller,
                    member.thread,
                    &request,
                    member.reach,
                    Some(batch),
                    None,
                )
                .await
            else {
                continue;
            };
            selected.push(ConfirmationMember {
                stored: StoredMember {
                    request,
                    ..member.clone()
                },
                thread,
                selected: prepared,
                actor: caller.user(),
                transaction,
            });
        }
        Ok(selected)
    }
    /// Each original native supplier runs once, outside repeatable action preparation for HTTP callers.
    pub async fn assemble_batch_confirmation(
        &self,
        conn: &mut orch_db::Conn,
        prepared: &BatchConfirmation,
        sources: Option<&[source::Preparation]>,
    ) -> Result<Vec<Option<Draft>>, PlanError> {
        Box::pin(self.assemble_confirmation_members(conn, &prepared.members, sources)).await
    }
    pub(super) async fn assemble_confirmation_members(
        &self,
        conn: &mut orch_db::Conn,
        members: &[ConfirmationMember],
        sources: Option<&[source::Preparation]>,
    ) -> Result<Vec<Option<Draft>>, PlanError> {
        if sources.is_some_and(|sources| sources.len() != members.len()) {
            return Err(
                EngineError::unreadable("The native confirmation source count changed.").into(),
            );
        }
        let mut drafts = Vec::new();
        for (index, member) in members.iter().enumerate() {
            match self
                .assemble_prepared_fork_with_source(
                    conn,
                    &member.selected,
                    sources.map(|sources| &sources[index]),
                )
                .await
            {
                Ok(draft) => drafts.push(Some(draft)),
                Err(error @ PlanError::Internal(EngineError::BeforeEffect(_))) => {
                    return Err(error);
                }
                Err(_) => drafts.push(None),
            }
        }
        Ok(drafts)
    }
    /// The native action keeps ownership of its commit and returns only the actual queued members.
    #[allow(
        clippy::too_many_arguments,
        reason = "the native transaction owner retains its original sources, measured operands and both effect cuts"
    )]
    pub async fn commit_batch_confirmation_with_source(
        &self,
        mut tx: orch_db::Tx,
        principal: &orch_authz::Principal,
        prepared: &BatchConfirmation,
        drafts: Vec<Option<Draft>>,
        sources: &[source::Origin],
        before: impl FnOnce() -> Result<(), EngineError> + Send,
        deadline: impl Fn() -> Result<(), EngineError> + Send + Sync,
    ) -> Result<(Id, Vec<JobRow>), PlanError> {
        let transaction: i64 = sqlx::query_scalar("SELECT txid_current()::bigint")
            .fetch_one(&mut *tx)
            .await
            .map_err(orch_db::DbError::from)?;
        if transaction != prepared.transaction
            || principal.user() != prepared.actor
            || fork_rows::preflight(&mut tx, prepared.preflight.id)
                .await?
                .as_ref()
                != Some(&prepared.preflight)
        {
            return Err(EngineError::unreadable(
                "The native batch changed its actor, transaction or kept plan.",
            )
            .into());
        }
        let budget = || {
            deadline()?;
            for origin in sources {
                origin.deadline(self.inner.clock.as_ref(), principal.caller())?;
            }
            Ok(())
        };
        let jobs = crate::notifications::operation_scope(
            Some(prepared.batch),
            self.queue_confirmation_members(
                &mut tx,
                principal.caller(),
                &prepared.members,
                drafts,
                Some(sources),
                Queue {
                    batch: prepared.batch,
                    preflight: prepared.preflight.id,
                    new_environment: matches!(
                        prepared.stored.body.environment,
                        Some(ForkEnvironment::New { .. })
                    ),
                },
                before,
                &budget,
            ),
        )
        .await?;
        if orch_db::idempotency::creation::captures(Some(principal.user()), "fork_batch") {
            let roots = match prepared.stored.body.all_of_environment {
                Some(id) => vec![orch_access_contract::decisions::ObjectRef {
                    kind: orch_access_contract::permissions::ObjectKind::Environment,
                    id,
                }],
                None => jobs
                    .iter()
                    .filter_map(|row| row.node_id)
                    .map(|id| orch_access_contract::decisions::ObjectRef {
                        kind: orch_access_contract::permissions::ObjectKind::Thread,
                        id,
                    })
                    .collect(),
            };
            orch_db::idempotency::creation::observe_with(
                &mut tx,
                Some(principal.user()),
                "fork_batch",
                prepared.batch,
                serde_json::json!({"queued":jobs.len(),"roots":roots}),
            )
            .await?;
        }
        Box::pin(self.finish_batch_confirmation(
            tx,
            principal,
            &prepared.body,
            prepared.batch,
            jobs,
            budget,
        ))
        .await
    }
    /// The native first-job INSERT and shared maker checkpoint consume the selected transaction operands.
    #[allow(
        clippy::too_many_arguments,
        reason = "one native queue owns its selected rows, measured operands, original sources and effect callbacks"
    )]
    pub(super) async fn queue_confirmation_members(
        &self,
        conn: &mut orch_db::Tx,
        caller: &Caller,
        members: &[ConfirmationMember],
        drafts: Vec<Option<Draft>>,
        sources: Option<&[source::Origin]>,
        queue: Queue,
        before: impl FnOnce() -> Result<(), EngineError> + Send,
        deadline: impl Fn() -> Result<(), EngineError> + Send + Sync,
    ) -> Result<Vec<JobRow>, PlanError> {
        if drafts.len() != members.len()
            || sources.is_some_and(|sources| sources.len() != members.len())
        {
            return Err(
                EngineError::unreadable("The native confirmation operand count changed.").into(),
            );
        }
        let transaction: i64 = sqlx::query_scalar("SELECT txid_current()::bigint")
            .fetch_one(&mut **conn)
            .await
            .map_err(orch_db::DbError::from)?;
        let mut jobs = Vec::new();
        let mut first = Some(before);
        for (index, (member, draft)) in members.iter().zip(drafts).enumerate() {
            if transaction != member.transaction || caller.user() != member.actor {
                return Err(EngineError::unreadable(
                    "The native batch member changed its actor or transaction.",
                )
                .into());
            }
            let Some(mut draft) = draft else { continue };
            if !draft.is_clear() {
                continue;
            }
            let mut request = member.selected.request().clone();
            request.history = Some(member.selected.history().clone());
            if draft.request != request
                || draft.plan.source.id != member.thread.node_id
                || draft.plan.result != ForkResult::Replace
                || draft.plan.to.agent.id != member.selected.agent().id
                || draft.plan.to.provider.id != member.selected.provider().id
                || draft.plan.to.directory != member.selected.directory()
                || draft.source_scope
                    != crate::fork_plan::SourceScope::of(&member.thread, member.stored.reach)
            {
                return Err(EngineError::unreadable(
                    "The native batch changed its selected operands.",
                )
                .into());
            }
            if sources.is_none() {
                // Existing internal callers retain the original request-default behavior.
                draft.request = member.stored.request.clone();
            }
            deadline()?;
            let before = first.take();
            jobs.push(
                self.start_prepared_fork_job(
                    conn,
                    caller,
                    &member.thread,
                    draft,
                    Some(queue.batch),
                    queue.preflight,
                    sources.map(|sources| &sources[index]),
                    || {
                        deadline()?;
                        if let Some(before) = before {
                            before()?;
                        }
                        Ok(())
                    },
                )
                .await?,
            );
        }
        if jobs.is_empty() {
            return Err(PlanError::NothingExecutable);
        }
        if queue.new_environment {
            let maker = jobs.first().ok_or(PlanError::NothingExecutable)?.id;
            for job in &mut jobs {
                let patch = serde_json::json!({"batch_environment_job":maker});
                job_rows::merge_plan(conn, job.id, &patch, self.inner.clock.now()).await?;
                if let Some(plan) = job.plan.as_object_mut() {
                    plan.insert("batch_environment_job".into(), serde_json::json!(maker));
                }
            }
        }
        deadline()?;
        Ok(jobs)
    }
}
#[derive(Clone, Copy)]
pub(super) struct Selection {
    pub locking: bool,
    pub declared: bool,
}

pub(super) struct Queue {
    pub batch: Id,
    pub preflight: Id,
    pub new_environment: bool,
}

#[cfg(test)]
mod tests;

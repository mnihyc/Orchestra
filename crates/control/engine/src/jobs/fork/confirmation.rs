//! Pure confirmation selection and the original job writer's guarded first insertion.
use super::*;
use crate::fork_plan::{PreparedFork, StoredPlan};

/// The same kept request and native source/target, on its actual actor and transaction.
#[derive(Clone)]
pub struct Confirmation {
    preflight: fork_rows::PreflightRow,
    stored: StoredPlan,
    thread: threads::ThreadRow,
    selected: PreparedFork,
    actor: Id,
    transaction: i64,
    notify: Option<bool>,
}
impl Confirmation {
    pub fn selected(&self) -> &PreparedFork {
        &self.selected
    }
    pub fn moving(&self) -> Option<&crate::fork_plan::moving::Review> {
        self.stored.moving.as_ref()
    }
    pub fn preflight_id(&self) -> Id {
        self.preflight.id
    }
    pub fn preserves(&self, current: &Self) -> bool {
        self.actor == current.actor
            && self.preflight == current.preflight
            && self.stored == current.stored
            && self.notify == current.notify
            && self.selected.preserves(&current.selected)
    }
}
fn requested(
    stored: &StoredPlan,
    body: &ForkCreateBody,
) -> Result<(ForkRequest, crate::binding::BirthAgentAccess), PlanError> {
    let agent_access = match &stored.agent_access {
        Some(crate::binding::BirthAgentAccess::ResolvePreset) => return Err(PlanError::Expired),
        Some(choice) => choice.clone(),
        None if stored.request.result == ForkResult::Replace => {
            crate::binding::BirthAgentAccess::nothing()
        }
        None => return Err(PlanError::Expired),
    };
    let mut request = stored.request.clone();
    // Confirmation keeps the displayed observation; it never starts another explicit fingerprint.
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
    Ok((request, agent_access))
}
impl Engine {
    /// The kept native header chooses its variant before scoped native preparation.
    pub async fn fork_confirmation_result(
        &self,
        conn: &mut orch_db::Conn,
        caller: &Caller,
        thread: Id,
        body: &ForkCreateBody,
    ) -> Result<ForkResult, PlanError> {
        let (_, stored) = self
            .load_fork_plan(conn, caller, body.preflight_id, Some(thread))
            .await?;
        Ok(stored.request.result)
    }
    /// The kept native body and closed ending purpose, before choosing any new supplier.
    pub async fn fork_confirmation_input(
        &self,
        conn: &mut orch_db::Conn,
        caller: &Caller,
        thread: Id,
        body: &ForkCreateBody,
    ) -> Result<(ForkRequest, Option<crate::fork_plan::moving::Review>), PlanError> {
        let (_, stored) = self
            .load_fork_plan(conn, caller, body.preflight_id, Some(thread))
            .await?;
        let request = requested(&stored, body)?.0;
        Ok((request, stored.moving))
    }
    /// Native header selection without row locks, mutation or supplier IO, for once-only observation.
    pub async fn select_fork_confirmation(
        &self,
        conn: &mut orch_db::Conn,
        caller: &Caller,
        thread: Id,
        body: &ForkCreateBody,
    ) -> Result<Confirmation, PlanError> {
        self.fork_confirmation_on(conn, caller, thread, body, false)
            .await
    }
    /// Native serialized confirmation, still effect-free and with no supplier IO.
    pub async fn prepare_fork_confirmation(
        &self,
        conn: &mut orch_db::Conn,
        caller: &Caller,
        thread: Id,
        body: &ForkCreateBody,
    ) -> Result<Confirmation, PlanError> {
        self.fork_confirmation_on(conn, caller, thread, body, true)
            .await
    }
    async fn fork_confirmation_on(
        &self,
        conn: &mut orch_db::Conn,
        caller: &Caller,
        id: Id,
        body: &ForkCreateBody,
        locking: bool,
    ) -> Result<Confirmation, PlanError> {
        let (preflight, stored) = self
            .load_fork_plan(conn, caller, body.preflight_id, Some(id))
            .await?;
        let thread = if locking {
            threads::lock(conn, id).await?
        } else {
            threads::thread(conn, id).await?
        }
        .filter(|row| row.deleted_at.is_none())
        .ok_or(PlanError::NotFound)?;
        if !stored
            .source_scope
            .as_ref()
            .is_some_and(|scope| scope.matches(&thread))
        {
            return Err(PlanError::Expired);
        }
        if let Some(job) = crate::locks::held_by(conn, id).await? {
            return Err(PlanError::Held(job));
        }
        let (request, access) = requested(&stored, body)?;
        let selected = self
            .prepare_fork_in(
                conn,
                caller,
                id,
                &request,
                crate::fork_plan::Reach::Seen,
                None,
                Some(access),
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
        Ok(Confirmation {
            preflight,
            stored,
            thread,
            selected,
            actor: caller.user(),
            transaction,
            notify: body.notify,
        })
    }
    /// The original native job insert, history cut, state/lock and preflight removal, committed together.
    #[allow(
        clippy::too_many_arguments,
        reason = "the native owner retains both its first insert and final commit callbacks"
    )]
    pub async fn commit_fork_confirmation(
        &self,
        tx: orch_db::Tx,
        caller: &Caller,
        prepared: &Confirmation,
        draft: Draft,
        before: impl FnOnce() -> Result<(), EngineError> + Send,
        deadline: impl Fn() -> Result<(), EngineError> + Send,
    ) -> Result<JobRow, PlanError> {
        self.commit_confirmation_on(tx, caller, prepared, draft, None, before, deadline)
            .await
    }
    /// The same native writer retains the actual admitted operation beside its confirmed history source.
    #[allow(
        clippy::too_many_arguments,
        reason = "the native writer owns its original source and both effect callbacks"
    )]
    pub async fn commit_fork_confirmation_with_source(
        &self,
        tx: orch_db::Tx,
        caller: &Caller,
        prepared: &Confirmation,
        draft: Draft,
        origin: &crate::fork_plan::operation::Origin,
        before: impl FnOnce() -> Result<(), EngineError> + Send,
        deadline: impl Fn() -> Result<(), EngineError> + Send,
    ) -> Result<JobRow, PlanError> {
        self.commit_confirmation_on(tx, caller, prepared, draft, Some(origin), before, deadline)
            .await
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "one writer keeps native operands and the actual source/effect callbacks together"
    )]
    async fn commit_confirmation_on(
        &self,
        mut tx: orch_db::Tx,
        caller: &Caller,
        prepared: &Confirmation,
        draft: Draft,
        origin: Option<&crate::fork_plan::operation::Origin>,
        before: impl FnOnce() -> Result<(), EngineError> + Send,
        deadline: impl Fn() -> Result<(), EngineError> + Send,
    ) -> Result<JobRow, PlanError> {
        if prepared.stored.moving.as_ref() != origin.and_then(|origin| origin.input().moving()) {
            return Err(EngineError::unreadable(
                "the confirmation has another native ending purpose",
            )
            .into());
        }

        let transaction: i64 = sqlx::query_scalar("SELECT txid_current()::bigint")
            .fetch_one(&mut *tx)
            .await
            .map_err(orch_db::DbError::from)?;
        if transaction != prepared.transaction
            || caller.user() != prepared.actor
            || draft.request != *prepared.selected.request()
            || draft.plan.source.id != prepared.thread.node_id
            || draft.plan.result != prepared.selected.request().result
            || draft.plan.to.agent.id != prepared.selected.agent().id
            || draft.plan.to.provider.id != prepared.selected.provider().id
            || draft.plan.to.directory != prepared.selected.directory()
            || draft.source_scope
                != crate::fork_plan::SourceScope::of(
                    &prepared.thread,
                    crate::fork_plan::Reach::Seen,
                )
        {
            return Err(EngineError::unreadable(
                "the native confirmation changed its actor, transaction or selected operands",
            )
            .into());
        }
        if !draft.is_clear() {
            return Err(PlanError::Blocked(Box::new(draft)));
        }
        let origin = origin
            .map(|origin| origin.accepted_from(caller, None))
            .transpose()?;
        let authority = match &origin {
            Some(origin) => source::Authority::Managed { caller, origin },
            None => source::Authority::Direct(caller),
        };
        let job = self
            .start_fork_job_with_authority(
                &mut tx,
                super::ForkStart {
                    authority,
                    thread: &prepared.thread,
                    draft,
                    batch: None,
                    preflight: prepared.preflight.id,
                },
                || {
                    if let Some(origin) = &origin {
                        origin.deadline(self.inner.clock.as_ref(), caller)?;
                    }
                    before()
                },
            )
            .await?;
        if prepared.stored.moving.is_some() {
            let audience = prepared.stored.move_people.as_ref().ok_or_else(|| {
                EngineError::unreadable("the native Move has no reviewed notice audience")
            })?;
            let notify = match prepared.notify {
                Some(choice) => choice,
                None => {
                    crate::notifications::operation_default(
                        &mut tx,
                        orch_settings::Place::Node(prepared.thread.node_id),
                        true,
                    )
                    .await?
                }
            };
            if notify && audience.count > u64::from(crate::fork_plan::moving::NOTICE_LIMIT) {
                return Err(PlanError::invalid(
                    "notify",
                    "the reviewed audience exceeds the explicit notice limit; choose not to notify",
                ));
            }
            orch_db::queries::jobs::merge_plan(&mut tx,job.id,&serde_json::json!({"thread_move_notice":crate::fork_plan::moving::Notice{notify,people:audience.people.clone()}}),self.inner.clock.now()).await?;
        }
        fork_rows::forget_preflight(&mut tx, prepared.preflight.id).await?;
        deadline()?;
        if let Some(origin) = &origin {
            origin.deadline(self.inner.clock.as_ref(), caller)?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.kick();
        self.wake(prepared.thread.node_id);
        Ok(job)
    }
}

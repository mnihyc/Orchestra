//! Actual retained fork authority on the same native source, destination and history gates.
use super::*;
use crate::{access::ResourceAccess, fork_plan::operation::Origin};
use orch_authz::{Caller, Principal};
use orch_db::Conn;
use orch_types::{Clock, Hash};

/// A native job association, never an Engine or reusable caller admission.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preparation {
    job: Id,
    origin: Origin,
    recipe: Hash,
}
impl std::fmt::Debug for Preparation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ForkPreparation([private])")
    }
}
/// The native actor and earliest accepted request/key bound for one current preparation.
pub struct Checkpoint {
    principal: Principal,
    origin: Origin,
    preparation: Hash,
    stored: Hash,
    transaction: i64,
}
impl Checkpoint {
    pub fn caller(&self) -> &Caller {
        self.principal.caller()
    }
    pub fn principal(&self) -> &Principal {
        &self.principal
    }
    pub fn until(&self) -> Option<Time> {
        self.origin.until()
    }
    pub fn origin(&self) -> &Origin {
        &self.origin
    }
    pub fn check(&self, clock: &dyn Clock) -> Result<(), EngineError> {
        self.origin.deadline(clock, self.principal.caller())
    }
}
fn refusal() -> EngineError {
    EngineError::BeforeEffect(Box::new(crate::operation_scope::continuing::Error::Refused))
}
fn native_error(error: Stop) -> EngineError {
    match error {
        Stop::Failed(error) if error.code == INTERNAL => EngineError::unreadable(error.message),
        _ => refusal(),
    }
}
impl Preparation {
    /// The real native row supplies both its immutable recipe and exact original operation.
    pub fn from_job(row: &JobRow) -> Result<Option<Self>, EngineError> {
        if row.kind != KIND {
            return Ok(None);
        }
        let source = source::retained(&row.plan).map_err(native_error)?;
        let Some(origin) = source.operation else {
            return Ok(None);
        };
        let recipe: Recipe = serde_json::from_value(row.plan.clone())?;
        if row.node_id != Some(recipe.source_thread)
            || row.batch_id != recipe.batch_id
            || row.created_by != Some(recipe.by)
            || source.cut_id != row.id
            || source.thread != recipe.source_thread
            || !origin.matches_job(
                source.thread,
                &recipe.request,
                source.reach,
                recipe.batch_id,
            )
            || origin.captured().user() != recipe.by
        {
            return Err(refusal());
        }
        Ok(Some(Self {
            job: row.id,
            origin,
            recipe: crate::jobs::command_identity::hash(&serde_json::to_value(recipe)?)?,
        }))
    }
    pub fn job(&self) -> Id {
        self.job
    }
    pub fn origin(&self) -> &Origin {
        &self.origin
    }
    /// The real maker checkpoint binds a provisioned child before any target Thread exists.
    pub(crate) async fn current_provision(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        access: &dyn ResourceAccess,
        environment: Id,
    ) -> Result<Checkpoint, EngineError> {
        let checkpoint = self.current(conn, clock, access).await?;
        let parent = orch_db::queries::jobs::job(conn, self.job)
            .await?
            .ok_or_else(refusal)?;
        let recipe: Recipe = serde_json::from_value(parent.plan.clone())?;
        let Some(ForkEnvironment::New { new }) = &recipe.request.environment else {
            return Err(refusal());
        };
        if recipe.to_environment.is_some()
            || !matches!(recipe.forkplan.to.environment, ForkPlanTarget::Made { .. })
            || member::<Id>(&parent.plan, made::ENVIRONMENT) != Some(environment)
            || parent
                .plan
                .get("batch_environment_job")
                .is_some_and(|maker| maker != &json!(self.job))
        {
            return Err(refusal());
        }
        let retained = source::retained(&parent.plan).map_err(native_error)?;
        let child = hosting_rows::environment(conn, environment)
            .await?
            .ok_or_else(refusal)?;
        if child.owner_id != recipe.by
            || child.home_node_id != new.home_node_id.unwrap_or(retained.topic)
            || new
                .template_id
                .is_some_and(|template| child.template_id != Some(template))
            || child.deleted_at.is_some()
        {
            return Err(refusal());
        }
        checkpoint.check(clock)?;
        Ok(checkpoint)
    }
    /// The same native fork provider binds the existing destination selected by its recipe.
    pub(crate) async fn current_wake(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        access: &dyn ResourceAccess,
        environment: Id,
    ) -> Result<Checkpoint, EngineError> {
        let checkpoint = self.current(conn, clock, access).await?;
        let parent = orch_db::queries::jobs::job(conn, self.job)
            .await?
            .ok_or_else(refusal)?;
        let recipe: Recipe = serde_json::from_value(parent.plan)?;
        if recipe.to_environment != Some(environment)
            || !matches!(&recipe.forkplan.to.environment, ForkPlanTarget::Existing(target) if target.id == environment)
            || matches!(
                &recipe.request.environment,
                Some(ForkEnvironment::New { .. })
            )
        {
            return Err(refusal());
        }
        checkpoint.check(clock)?;
        Ok(checkpoint)
    }
    pub(crate) fn accepted_snapshot(
        &self,
        origin: orch_authz::CapturedOperation,
        until: Option<Time>,
    ) -> Result<Self, EngineError> {
        Ok(Self {
            origin: self.origin.accepted_snapshot(origin, until)?,
            ..self.clone()
        })
    }
    pub(crate) async fn current_provider(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        access: &dyn ResourceAccess,
        provider: Id,
        machine: Option<Id>,
    ) -> Result<Checkpoint, EngineError> {
        let checkpoint = self.current(conn, clock, access).await?;
        let row = orch_db::queries::jobs::job(conn, self.job)
            .await?
            .ok_or_else(refusal)?;
        let recipe: Recipe = serde_json::from_value(row.plan)?;
        if provider != recipe.forkplan.to.provider.id {
            return Err(refusal());
        }
        if let Some(machine) = machine {
            let row = resource_rows::provider(conn, provider)
                .await?
                .ok_or_else(refusal)?;
            if crate::leases::machine_of(&row) != Some(machine) {
                return Err(refusal());
            }
        }
        checkpoint.check(clock)?;
        Ok(checkpoint)
    }
    pub fn preserves(&self, current: &Self) -> bool {
        self.job == current.job
            && self.recipe == current.recipe
            && self.origin.same_act(&current.origin)
    }
    pub fn accepted_at(&self, current: &Checkpoint) -> Result<Self, EngineError> {
        if current.preparation != crate::jobs::command_identity::hash(&serde_json::to_value(self)?)?
            || !self.origin.same_act(&current.origin)
        {
            return Err(refusal());
        }
        Ok(Self {
            origin: current.origin.clone(),
            ..self.clone()
        })
    }
    /// Retains the same accepted minimum on its locked native job; no later bound can overwrite it.
    pub async fn retain_on(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        checkpoint: &Checkpoint,
    ) -> Result<Self, EngineError> {
        if !orch_db::queries::transaction_guard::in_transaction(conn) {
            return Err(refusal());
        }
        let transaction: i64 = sqlx::query_scalar("SELECT txid_current()::bigint")
            .fetch_one(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
        if transaction != checkpoint.transaction {
            return Err(refusal());
        }
        let accepted = self.accepted_at(checkpoint)?;
        let row = orch_db::queries::jobs::lock(conn, self.job)
            .await?
            .ok_or_else(refusal)?;
        let current = Self::from_job(&row)?.ok_or_else(refusal)?;
        if crate::jobs::command_identity::hash(&serde_json::to_value(current)?)?
            != checkpoint.stored
        {
            return Err(refusal());
        }
        checkpoint.check(clock)?;
        source::retain_operation(conn, &row, checkpoint.caller(), checkpoint.origin(), clock)
            .await
            .map_err(native_error)?;
        checkpoint.check(clock)?;
        Ok(accepted)
    }
    /// Retains the native writer's actor -> job -> source-thread order before a command row.
    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        orch_db::queries::transaction_guard::lock_native_actor(conn, self.origin.captured().user())
            .await?;
        job_rows_lock(conn, self.job).await?;
        // This fence precedes command locks, including reconciliation of an accepted receipt.
        // Absence grants nothing: current() separately reloads and authorizes the live source.
        match crate::fork_plan::operation::lock(conn, self.origin.input()).await {
            Ok(()) | Err(crate::operation_scope::continuing::Error::Refused) => {}
            Err(crate::operation_scope::continuing::Error::Native(error)) => return Err(error),
            Err(crate::operation_scope::continuing::Error::Binding(error)) => {
                return Err(EngineError::unreadable(error));
            }
        }
        Ok(())
    }
    /// Shared startup/forwarding consumers call the SAME native source/destination authorizer.
    pub async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        access: &dyn ResourceAccess,
    ) -> Result<Checkpoint, EngineError> {
        self.lock_source(conn).await?;
        let row = orch_db::queries::jobs::job(conn, self.job)
            .await?
            .ok_or_else(refusal)?;
        if !row.is_open()
            || row.cancel_requested()
            || crate::jobs::runner::abort_reason(&row)?.is_some()
        {
            return Err(refusal());
        }
        let current = Self::from_job(&row)?.ok_or_else(refusal)?;
        if self.recipe != current.recipe || !self.origin.same_act(&current.origin) {
            return Err(refusal());
        }
        let recipe: Recipe = serde_json::from_value(row.plan.clone())?;
        let mut source = source::retained(&row.plan).map_err(native_error)?;
        source.operation = Some(self.origin.clone());
        let target =
            threads::thread(conn, recipe.new_thread.unwrap_or(recipe.source_thread)).await?;
        let authorized =
            source::authorize_on(conn, &row, &recipe, &source, target.as_ref(), clock, access)
                .await
                .map_err(native_error)?;
        let source::Authorized::Managed { principal, origin } = authorized else {
            return Err(refusal());
        };
        let transaction = sqlx::query_scalar("SELECT txid_current()::bigint")
            .fetch_one(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
        let checkpoint = Checkpoint {
            origin: *origin,
            principal,
            preparation: crate::jobs::command_identity::hash(&serde_json::to_value(self)?)?,
            stored: crate::jobs::command_identity::hash(&serde_json::to_value(current)?)?,
            transaction,
        };
        checkpoint.check(clock)?;
        Ok(checkpoint)
    }
    /// A startup key must be the actual native selected target on its current physical machine.
    pub async fn current_harness(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        access: &dyn ResourceAccess,
        key: HarnessKey,
    ) -> Result<Checkpoint, EngineError> {
        let checkpoint = self.current(conn, clock, access).await?;
        let row = orch_db::queries::jobs::job(conn, self.job)
            .await?
            .ok_or_else(refusal)?;
        let recipe: Recipe = serde_json::from_value(row.plan.clone())?;
        if key.provider != recipe.forkplan.to.provider.id
            || key.agent != recipe.forkplan.to.agent.id
        {
            return Err(refusal());
        }
        let environment = recipe
            .to_environment
            .or_else(|| member::<Id>(&row.plan, made::ENVIRONMENT))
            .ok_or_else(refusal)?;
        let environment = environment_rows::environment(conn, environment)
            .await?
            .ok_or_else(refusal)?;
        let provider = resource_rows::provider(conn, key.provider)
            .await?
            .ok_or_else(refusal)?;
        let arrangement: orch_agent_contract::descriptor::ArrangementType =
            crate::sessions::convert(&recipe.forkplan.to.arrangement)?;
        let machine = harnesses::machine_of(
            Some(arrangement.as_str()),
            &provider,
            environment.machine_id,
        );
        if machine != Some(key.machine) {
            return Err(refusal());
        }
        checkpoint.check(clock)?;
        Ok(checkpoint)
    }
}
async fn job_rows_lock(conn: &mut Conn, id: Id) -> Result<(), EngineError> {
    orch_db::queries::jobs::lock(conn, id).await?;
    Ok(())
}

/// The managed switch's existing database decisions use the same actual native source.
pub(super) async fn cancel_approvals(job: &Job, thread: Id) -> Result<(), Stop> {
    let row = job.row().await?;
    let Some(source) = Preparation::from_job(&row)? else {
        return Ok(job.engine().cancel_pending_approvals(thread).await?);
    };
    if source.origin().input().thread() != thread {
        return Err(refusal().into());
    }
    let engine = job.engine();
    let mut tx = engine.db().begin().await?;
    let checkpoint = source
        .current(
            &mut tx,
            engine.inner.clock.as_ref(),
            engine.inner.access.as_ref(),
        )
        .await?;
    source
        .retain_on(&mut tx, engine.inner.clock.as_ref(), &checkpoint)
        .await?;
    let finished = {
        let mut unit = Unit::with_first_effect(&mut tx, engine.inner.clock.now(), || {
            checkpoint.check(engine.inner.clock.as_ref())
        });
        crate::locks::cancel_pending_approvals_in(&mut unit, thread).await?;
        unit.finish().await?
    };
    checkpoint.check(engine.inner.clock.as_ref())?;
    engine.commit(tx, finished).await?;
    Ok(())
}
/// An already recorded interrupt is driven as its native receipt; a fresh decision needs its source.
pub(super) async fn interrupt(
    job: &Job,
    thread: Id,
    turn: &threads::TurnRow,
    by: Id,
) -> Result<(), Stop> {
    let row = job.row().await?;
    let Some(source) = Preparation::from_job(&row)? else {
        return Ok(job.engine().interrupt_for_change(thread, turn, by).await?);
    };
    if source.origin().input().thread() != thread || source.origin().captured().user() != by {
        return Err(refusal().into());
    }
    let engine = job.engine();
    if turn.interrupted_by.is_none() {
        let mut tx = engine.db().begin().await?;
        let checkpoint = source
            .current(
                &mut tx,
                engine.inner.clock.as_ref(),
                engine.inner.access.as_ref(),
            )
            .await?;
        source
            .retain_on(&mut tx, engine.inner.clock.as_ref(), &checkpoint)
            .await?;
        let finished = {
            let mut unit = Unit::with_first_effect(&mut tx, engine.inner.clock.now(), || {
                checkpoint.check(engine.inner.clock.as_ref())
            });
            super::interrupt_for_change_in(&mut unit, thread, turn, by).await?;
            unit.finish().await?
        };
        checkpoint.check(engine.inner.clock.as_ref())?;
        engine.commit(tx, finished).await?;
    }
    engine.drive_interrupt(thread).await?;
    Ok(())
}

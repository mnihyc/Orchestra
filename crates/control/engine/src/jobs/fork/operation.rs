//! Actual retained fork authority on the same native source, destination and history gates.
use super::*;
use crate::{access::ResourceAccess, fork_plan::operation::Origin};
use orch_authz::Caller;
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
    caller: Caller,
    origin: Origin,
    preparation: Hash,
}
impl Checkpoint {
    pub fn caller(&self) -> &Caller {
        &self.caller
    }
    pub fn until(&self) -> Option<Time> {
        self.origin.until()
    }
    pub fn origin(&self) -> &Origin {
        &self.origin
    }
    pub fn check(&self, clock: &dyn Clock) -> Result<(), EngineError> {
        self.origin.deadline(clock, &self.caller)
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
            || source.cut_id != row.id
            || source.thread != recipe.source_thread
            || origin.input().thread() != source.thread
            || origin.input().effective_request() != recipe.request
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
    /// Retains the native writer's actor -> job -> source-thread order before a command row.
    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        orch_db::queries::transaction_guard::lock_native_actor(conn, self.origin.captured().user())
            .await?;
        job_rows_lock(conn, self.job).await?;
        threads::lock(conn, self.origin.input().thread()).await?;
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
        let source::Authorized::Managed { caller, origin } = authorized else {
            return Err(refusal());
        };
        let checkpoint = Checkpoint {
            origin,
            caller,
            preparation: crate::jobs::command_identity::hash(&serde_json::to_value(self)?)?,
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
    orch_db::queries::jobs::lock(conn, id)
        .await?
        .ok_or_else(refusal)?;
    Ok(())
}

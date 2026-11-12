//! Native observed operands and the sealed creator factory; future identities stay private.

use super::*;

use crate::key_operations::admission::Prepared;

use crate::registry::OperationFuture;

use orch_api_contract::adoption_sources::Source;

use orch_engine::adoption::PreparedAdoption;

use orch_engine::binding::Birth;

use orch_engine::binding::CreatedThread;

use orch_types::Id;
pub(crate) struct Adopt;

pub(crate) struct Staged {
    pub prepared: Arc<PreparedAdoption>,
    pub created: CreatedThread,
    pub birth: Birth,
    pub plan: Prepared,
}

impl native::effects::Effect for Adopt {
    type Source = Source;
    type Query = ();
    type Body = ThreadAdoptBody;
    type Value = Arc<Fetched>;
    type Output = ();
    fn declaration(_: &Arc<Fetched>) -> &'static Declaration<Source> {
        &declarations::ADOPT_PREPARATION
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Caller,
        (): (),
        body: ThreadAdoptBody,
    ) -> BoxFuture<'a, ApiResult<(Arc<Fetched>, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let adoption = adoption_of(ctx, body)?;
            let fetched = native::effects::initial_value::<Adopt>(ctx)?;
            if fetched.adoption() != &adoption {
                return Err(ApiError::internal(
                    "the native adoption observation changed its request",
                ));
            }
            let facts = source::initial_facts(conn, &adoption)
                .await
                .map_err(ApiError::engine)?;
            // These genuine known facts cannot admit a main action with unresolved creator roles.
            Ok((fetched, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Arc<Fetched>,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { super::require(ctx, conn, principal, value.adoption()).await })
    }
}

/// Select only this owner's sealed creator prefix after the real once-only native supplier.
pub(crate) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: ThreadAdoptBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let plan = native::effects::prepare_entry::<Adopt>(ctx, conn, caller, (), body).await?;
        plan.adoption_creation_prefix()
    })
}

fn deadline(
    ctx: &Ctx,
    guard: &transaction_guard::Guard,
    caller: &Caller,
    until: Option<orch_types::Time>,
) -> ApiResult<()> {
    guard.check_deadline(ctx)?;
    if !caller.key_deadline_allows(ctx.app.clock.as_ref())
        || until.is_some_and(|end| ctx.app.clock.time() >= end)
    {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthInvalidToken,
            "the native adoption credential has ended",
        ));
    }
    Ok(())
}

/// Consume the actual private creator outcome, import its native record and commit them together.
pub(crate) async fn create(
    ctx: Ctx,
    (): (),
    body: ThreadAdoptBody,
) -> ApiResult<orch_api_contract::nodes::ThreadAdoptResponse> {
    use orch_api_contract::nodes::{ThreadAdoptReceipt, ThreadAdoptResponse};
    let (current, mut tx, guard) =
        crate::key_operations::creation_stage::begin_adoption(&ctx, &body).await?;
    let staged = guard.operation_data::<Staged>()?;
    let constrained = guard
        .principal()
        .caller()
        .intersect_restrictions(staged.prepared.principal().caller())
        .map_err(ApiError::internal)?;
    let accepted = staged
        .plan
        .shared()
        .admit(&constrained, false)
        .map_err(crate::key_operations::admission::binding_error)?;
    let principal = crate::middleware::authorization::load_principal(
        &mut tx,
        accepted.clone(),
        current.app.clock.time(),
    )
    .await?;
    let principal = crate::sign_in::second_step::hold(&mut tx, current.user()?, principal).await?;
    let until = staged.prepared.record_until();
    let write = current
        .app
        .engine
        .write_adoption_in(
            &mut tx,
            &principal,
            &staged.prepared,
            &staged.created,
            || {
                deadline(&current, &guard, &accepted, until)
                    .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))
            },
        )
        .await;
    let write = match write {
        Ok(write) => write,
        Err(error) => return Err(problem(&current, &accepted, &mut tx, error).await),
    };
    creation::observed(
        &mut tx,
        current.user()?.id(),
        staged.prepared.new_thread().topic,
        write.outcome(),
    )
    .await?;
    deadline(&current, &guard, &accepted, until)?;
    let outcome = current
        .app
        .engine
        .commit_adoption(tx, write)
        .await
        .map_err(ApiError::engine)?;
    let receipt = ThreadAdoptReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        thread_id: outcome.created.thread,
        topic_id: staged.prepared.new_thread().topic,
        segment_id: outcome.created.segment,
        history_items: outcome.history_items,
    };
    crate::key_operations::publication::receipt(&current, &receipt)?;
    current.publication.retain_deadline(until)?;
    match readback::render(
        &current,
        &accepted,
        readback::Input {
            prepared: Arc::clone(&staged.prepared),
            birth: staged.birth.clone(),
            created: outcome.created,
        },
    )
    .await?
    {
        Some(thread) => Ok(ThreadAdoptResponse::full(thread)),
        None => Ok(ThreadAdoptResponse::Receipt(receipt)),
    }
}

impl native::effects::Initial for Adopt {
    type Input = Adoption;
    type Fetched = Fetched;
    fn declaration(_: &Adoption) -> &'static Declaration<Source> {
        &declarations::ADOPT_PREPARATION
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Caller,
        (): (),
        body: ThreadAdoptBody,
    ) -> BoxFuture<'a, ApiResult<(Adoption, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let adoption = adoption_of(ctx, body)?;
            let facts = source::initial_facts(conn, &adoption)
                .await
                .map_err(ApiError::engine)?;
            Ok((adoption, facts))
        })
    }
    fn require_initial<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Adoption,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { super::require(ctx, conn, principal, input).await })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Adoption,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move { super::fetch(ctx, caller, input).await })
    }
}

/// Only the shared private creator stage may call this native database factory.
/// Its once-only native observation is already cached; no RPC or store read is replayed.
pub(crate) fn complete_staged<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    topic: Id,
    body: &'a ThreadAdoptBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let adoption = adoption_of(ctx, body.clone())?;
        let fetched = native::effects::initial_value::<Adopt>(ctx)?;
        if topic != adoption.topic || fetched.adoption() != &adoption {
            return Err(ApiError::internal(
                "the native creator has another adoption request",
            ));
        }
        let principal = crate::middleware::authorization::load_principal(
            conn,
            caller.clone(),
            ctx.app.clock.time(),
        )
        .await?;
        let principal = crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await?;
        let prepared = match ctx
            .app
            .engine
            .prepare_adoption_birth_in(conn, &principal, fetched)
            .await
        {
            Ok(prepared) => Arc::new(prepared),
            Err(error) => return Err(problem(ctx, caller, conn, error).await),
        };
        let prefix = match orch_engine::operation_scope::creators::Prefix::prepare(
            conn,
            prepared.principal(),
            "ThreadAdopt",
            prepared.new_thread(),
            None,
        )
        .await
        {
            Ok(prefix) => prefix,
            Err(error) => return Err(problem(ctx, caller, conn, error.into()).await),
        };
        let (created, complete) = match ctx
            .app
            .engine
            .create_thread_staged_taking_in_transaction(
                conn,
                prefix,
                prepared.new_thread(),
                prepared.birth(),
                orch_types::new_id(),
                prepared.native_session_ref(),
            )
            .await
        {
            Ok(value) => value,
            Err(error) => return Err(problem(ctx, caller, conn, error.into()).await),
        };
        let mut birth = prepared.birth().clone();
        birth.agent_access = complete.agent_access;
        let plan = Prepared::from_shared(complete.plan);
        Ok(plan.clone().with_data(Staged {
            prepared,
            created,
            birth,
            plan,
        }))
    })
}

#[path = "creator/creation.rs"]
pub mod creation;

#[path = "creator/readback.rs"]
pub mod readback;

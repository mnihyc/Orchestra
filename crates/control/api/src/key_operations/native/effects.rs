//! Typed native preparation and application on the actual guarded connection.
//!
//! Preparation is effect-free and may repeat. It must hold the native writer's complete
//! serialization footprint before producing operands. The native apply method consumes
//! those same operands and invokes the opaque check at its existing first-effect seam.

use super::*;
use crate::key_operations::resolver::OperationData;
use crate::middleware::transaction_guard::Guard;
use crate::registry::{OperationFuture, OperationInitialFuture};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};

/// One native provider; operation/variant and role facts come from its real prepared intent.
pub(crate) trait Effect: Send + Sync + 'static {
    type Source: Eq + Send + Sync + 'static;
    type Query: Send + 'static;
    type Body: Send + 'static;
    type Value: Send + Sync + 'static;
    type Output: Send + 'static;

    fn declaration(value: &Self::Value) -> &'static Declaration<Self::Source>;

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: Self::Query,
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<Facts<Self::Source>>)>>;

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>>;
}

/// Native writers that apply on their caller's existing action connection.
pub(crate) trait Apply: Effect {
    /// Reuse the native writer and its already landed opaque first-effect callback.
    /// New native awaits that acquire an additional authority footprint require a
    /// native staged preparation; this interface does not make an arbitrary writer safe.
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Self::Output>>>;
}

/// Native runners that already own their transaction and its commit/cleanup semantics.
#[expect(
    dead_code,
    reason = "consumed by the environment_lifecycle cohort of provider-ceiling"
)]
pub(crate) trait ApplyOwned: Effect {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Self::Value,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Self::Output>>>;
}

struct Operands<F: Effect> {
    value: F::Value,
    response: OperationPlan,
}

/// The native no-op/effect distinction is explicit and preserves its ordinary semantics.
pub(crate) enum Outcome<T> {
    Unchanged(T),
    Applied(T),
}

/// A native writer's existing FnOnce callback consumes this at its actual first effect.
/// It grants nothing: full preparation/admission and ordinary authorization already ran
/// after the complete native lock footprint on this same guarded connection.
pub(crate) struct FirstEffect<'a> {
    ctx: &'a Ctx,
    guard: &'a Guard,
    checked: &'a AtomicBool,
}

impl FirstEffect<'_> {
    pub(crate) fn check(self) -> ApiResult<()> {
        self.guard.check_deadline(self.ctx)?;
        self.checked.store(true, Ordering::Release);
        Ok(())
    }
}

/// Typed resolver used for browser, legacy and scoped callers of the same native producer.
/// Query/body types need no Clone or Serialize; guarded checkpoints decode the original input.
pub(crate) fn prepare<'a, F: Effect>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: F::Query,
    body: F::Body,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let (value, facts) = F::prepare(ctx, conn, caller, query, body).await?;
        let response = bind_required(ctx, F::declaration(&value), &facts)?;
        let principal = preparation_principal(ctx, conn, caller, &response).await?;
        F::require(ctx, conn, &principal, &value).await?;
        Ok(response
            .clone()
            .with_data(Operands::<F> { value, response }))
    })
}

/// Actual accepted outcome, kept opaque until the domain supplies its native receipt/projector.
pub(crate) struct Completed<T> {
    pub ctx: Ctx,
    pub accepted: Caller,
    pub output: T,
}

/// Use inside the existing native action transaction. Its fresh guarded resolver supplies
/// the exact operands; caller-side stale request preparation is never an apply input.
pub(crate) async fn apply<F: Apply>(ctx: &Ctx, conn: &mut Conn) -> ApiResult<Completed<F::Output>> {
    let (ctx, guard) = transaction_guard::after_wait(ctx, conn).await?;
    let operands = guard.operation_data::<Operands<F>>()?;
    if operands.response.variant().operation != F::declaration(&operands.value).operation {
        return Err(declaration_error());
    }
    F::require(&ctx, conn, guard.principal(), &operands.value).await?;
    let checked = AtomicBool::new(false);
    let before = FirstEffect {
        ctx: &ctx,
        guard: &guard,
        checked: &checked,
    };
    let outcome = F::apply(&ctx, conn, guard.principal(), &operands.value, before).await?;
    finish(ctx, guard, &checked, outcome)
}

/// The native runner keeps ownership of its transaction, commit and accepted cleanup.
#[expect(
    dead_code,
    reason = "consumed by the environment_lifecycle cohort of provider-ceiling"
)]
pub(crate) async fn apply_owned<F: ApplyOwned>(
    ctx: &Ctx,
    mut tx: orch_db::Tx,
) -> ApiResult<Completed<F::Output>> {
    let (ctx, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    let operands = guard.operation_data::<Operands<F>>()?;
    if operands.response.variant().operation != F::declaration(&operands.value).operation {
        return Err(declaration_error());
    }
    F::require(&ctx, &mut tx, guard.principal(), &operands.value).await?;
    let checked = AtomicBool::new(false);
    let before = FirstEffect {
        ctx: &ctx,
        guard: &guard,
        checked: &checked,
    };
    let outcome = F::apply_owned(&ctx, tx, guard.principal(), &operands.value, before).await?;
    finish(ctx, guard, &checked, outcome)
}

fn finish<T>(
    ctx: Ctx,
    guard: Guard,
    checked: &AtomicBool,
    outcome: Outcome<T>,
) -> ApiResult<Completed<T>> {
    let output = match outcome {
        Outcome::Unchanged(output) => output,
        Outcome::Applied(output) => {
            if !checked.load(Ordering::Acquire) {
                return Err(ApiError::internal(
                    "the native effect did not consume its first-effect check",
                ));
            }
            output
        }
    };
    Ok(Completed {
        accepted: guard.principal().caller().clone(),
        ctx,
        output,
    })
}

/// Existing bounded initial IO, authorized before waiting, separate from retryable preparation.
#[expect(
    dead_code,
    reason = "consumed by the import_export_storage cohort of provider-ceiling"
)]
pub(crate) trait Initial: Effect {
    type Input: Send + 'static;
    type Fetched: Send + Sync + 'static;

    fn declaration(input: &Self::Input) -> &'static Declaration<Self::Source>;

    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: Self::Query,
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Self::Input, Vec<Facts<Self::Source>>)>>;

    fn require_initial<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
    ) -> BoxFuture<'a, ApiResult<()>>;

    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Self::Input,
    ) -> BoxFuture<'a, ApiResult<Self::Fetched>>;
}

#[expect(
    dead_code,
    reason = "consumed by the import_export_storage cohort of provider-ceiling"
)]
struct InitialValue<F: Initial> {
    value: Arc<F::Fetched>,
    family: PhantomData<F>,
}

/// The Registry's initial hook, once per logical request, never called by an action resolver.
/// No database connection/transaction is kept while the native initial supplier waits.
#[expect(
    dead_code,
    reason = "consumed by the import_export_storage cohort of provider-ceiling"
)]
pub(crate) fn initial<'a, F: Initial>(
    ctx: &'a Ctx,
    caller: &'a Caller,
    query: F::Query,
    body: F::Body,
) -> OperationInitialFuture<'a> {
    Box::pin(async move {
        let data = ctx
            .operation_initial
            .get_or_try_init(|| async move {
                let mut tx = ctx.app.db.begin_snapshot().await?;
                let identity =
                    transaction_guard::current_http_identity(ctx, &mut tx, HttpIdentityMode::Read)
                        .await?;
                let caller = identity
                    .caller
                    .intersect_restrictions(caller)
                    .map_err(ApiError::internal)?;
                let current =
                    ctx.for_operation_checkpoint(identity.user.clone(), identity.now.get());
                let (input, facts) = F::authorize(&current, &mut tx, &caller, query, body).await?;
                let known = bind_known(&current, <F as Initial>::declaration(&input), &facts)?;
                let principal = preparation_principal(&current, &mut tx, &caller, &known).await?;
                F::require_initial(&current, &mut tx, &principal, &input).await?;
                tx.commit().await.map_err(DbError::from)?;
                identity.check_deadline(&ctx.app)?;
                let accepted = principal.caller().clone();
                let value = F::fetch(&current, &accepted, input).await?;
                identity.check_deadline(&ctx.app)?;
                if !accepted.key_deadline_allows(ctx.app.clock.as_ref()) {
                    return Err(ApiError::new(
                        orch_api_contract::common::ErrorCode::AuthInvalidToken,
                        "the initial preparation key has ended",
                    ));
                }
                Ok(OperationData::new(InitialValue::<F> {
                    value: Arc::new(value),
                    family: PhantomData,
                }))
            })
            .await?;
        let _ = data.get::<InitialValue<F>>()?;
        Ok(())
    })
}

/// Native preparation sees only its own immutable once-only initial operand.
#[expect(
    dead_code,
    reason = "consumed by the import_export_storage cohort of provider-ceiling"
)]
pub(crate) fn initial_value<F: Initial>(ctx: &Ctx) -> ApiResult<Arc<F::Fetched>> {
    let data = ctx
        .operation_initial
        .get()
        .ok_or_else(|| ApiError::internal("the native initial operand is unavailable"))?;
    Ok(Arc::clone(&data.get::<InitialValue<F>>()?.value))
}

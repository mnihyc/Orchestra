//! Request-owned preparation before decoding resolves all business roles.
use super::*;
use crate::key_operations::native::effects::{self, CommonInitial};
use futures::future::BoxFuture;
use orch_authz::{Caller, CapturedCaller, Principal};
use orch_db::Conn;
use orch_engine::operation_scope::sources::common::Common;
use orch_types::{Clock, Time};
use std::{marker::PhantomData, sync::Arc};

/// Current native preparation operands, carrying no complete business admission.
pub(crate) struct Frame<I: CommonInitial, F: Source<Role = I::Source>> {
    principal: Principal,
    until: Option<Time>,
    opened: Opened<I, F>,
}
/// Immutable native association, never serializable as a queued operation.
pub(crate) struct Opened<I: CommonInitial, F: Source<Role = I::Source>> {
    caller: CapturedCaller,
    input: Arc<F::Input>,
    value: Arc<F::Value>,
    common: Common,
    initial: PhantomData<I>,
}
/// Inspect the prepared association in adapter tests without exposing extra runtime operands.
#[cfg(test)]
pub(crate) struct Parts<I: CommonInitial, F: Source<Role = I::Source>> {
    pub principal: Principal,
    pub input: Arc<F::Input>,
    pub value: Arc<F::Value>,
    pub opened: Opened<I, F>,
}
impl<I: CommonInitial, F: Source<Role = I::Source>> Frame<I, F> {
    #[cfg(test)]
    pub(crate) fn principal(&self) -> &Principal {
        &self.principal
    }
    #[cfg(test)]
    pub(crate) fn value(&self) -> &F::Value {
        &self.opened.value
    }
    pub(crate) fn check_deadline(&self, clock: &dyn Clock) -> ApiResult<()> {
        check(self.principal.caller(), self.until, clock)
    }
    pub(crate) fn into_opened(self) -> Opened<I, F> {
        self.opened
    }
    #[cfg(test)]
    pub(crate) fn into_parts(self) -> Parts<I, F> {
        Parts {
            principal: self.principal,
            input: Arc::clone(&self.opened.input),
            value: Arc::clone(&self.opened.value),
            opened: self.opened,
        }
    }
}
fn check(caller: &Caller, until: Option<Time>, clock: &dyn Clock) -> ApiResult<()> {
    if !caller.key_deadline_allows(clock) || until.is_some_and(|end| clock.time() >= end) {
        return Err(refused());
    }
    Ok(())
}
fn refused() -> ApiError {
    ApiError::new(
        orch_api_contract::common::ErrorCode::AuthzDenied,
        "the native initial preparation is no longer current",
    )
}

/// Same factual Source/ordinary gate/native locks as continuing preparation, without
/// capturing an unresolved operation. All observed ceilings stay on this request.
pub(crate) async fn prepare_initial_on<I: CommonInitial, F: Source<Role = I::Source>>(
    ctx: &Ctx,
    conn: &mut Conn,
    original: &Caller,
    input: F::Input,
) -> ApiResult<(Frame<I, F>, Deadline)> {
    prepare::<I, F>(
        ctx,
        conn,
        &CapturedCaller::capture(original),
        Arc::new(input),
        None,
    )
    .await
}
/// Recheck the same native input/value after a source, store or conversion wait.
pub(crate) async fn refresh_initial_on<I: CommonInitial, F: Source<Role = I::Source>>(
    ctx: &Ctx,
    conn: &mut Conn,
    opened: &Opened<I, F>,
) -> ApiResult<(Frame<I, F>, Deadline)> {
    prepare::<I, F>(
        ctx,
        conn,
        &opened.caller,
        Arc::clone(&opened.input),
        Some(opened),
    )
    .await
}
async fn load<I: CommonInitial, F: Source<Role = I::Source>>(
    ctx: &Ctx,
    conn: &mut Conn,
    original: &CapturedCaller,
    input: Arc<F::Input>,
) -> ApiResult<(Frame<I, F>, Deadline)> {
    if !orch_db::queries::transaction_guard::in_transaction(conn)
        || ctx.route.name != F::ROUTE.name
        || ctx.route.method != F::ROUTE.method
        || ctx.route.path != F::ROUTE.path
        || original.user() != ctx.authenticated_user()?.id()
    {
        return Err(refused());
    }
    orch_db::queries::transaction_guard::lock_action(conn).await?;
    orch_db::queries::transaction_guard::lock_native_actor(conn, original.user())
        .await?
        .ok_or_else(refused)?;
    let mut identity =
        transaction_guard::current_http_identity(ctx, conn, HttpIdentityMode::Read).await?;
    transaction_guard::guard_user(&identity.user)?;
    let restored = original
        .restore_current(conn, ctx.app.clock.as_ref())
        .await
        .map_err(authorization::restoration_error)?
        .ok_or_else(refused)?;
    if !admission::same_response_origin(ctx, &restored)? {
        return Err(refused());
    }
    let caller = identity
        .caller
        .intersect_restrictions(&restored)
        .map_err(ApiError::internal)?;
    let current = ctx.for_operation_checkpoint(identity.user.clone(), identity.now.get());
    let (value, facts) = F::prepare(conn, &caller, &input, ctx.app.clock.time())
        .await
        .map_err(source_error)?;
    if !I::CANDIDATES
        .iter()
        .any(|row| row.operation == F::declaration(&input, &value).operation)
    {
        return Err(refused());
    }
    let common = effects::bind_common::<I>(&current, &facts)?;
    let caller = common
        .preparation_caller(&caller)
        .map_err(admission::binding_error)?;
    let principal = authorization::load_principal(conn, caller, ctx.app.clock.time()).await?;
    let principal = crate::sign_in::second_step::hold(conn, &identity.user, principal).await?;
    F::require(conn, &principal, &input, &value)
        .await
        .map_err(source_error)?;
    identity.caller = principal.caller().clone();
    identity.check_deadline(&ctx.app)?;
    ctx.publication.retain_ceiling(principal.caller())?;
    let until = ctx.publication.retained_deadline()?;
    check(principal.caller(), until, ctx.app.clock.as_ref())?;
    let caller = original
        .narrowed_with(principal.caller())
        .map_err(ApiError::internal)?;
    let frame = Frame {
        principal,
        until,
        opened: Opened {
            caller,
            input,
            value: Arc::new(value),
            common,
            initial: PhantomData,
        },
    };
    Ok((
        frame,
        Deadline {
            app: ctx.app.clone(),
            identity,
        },
    ))
}
fn same<'a, I: CommonInitial, F: Source<Role = I::Source>>(
    conn: &'a mut Conn,
    current: &'a Frame<I, F>,
    earlier: &'a Opened<I, F>,
) -> BoxFuture<'a, ApiResult<()>> {
    Box::pin(async move {
        if !current.opened.common.covers_common(&earlier.common)
            || !earlier.common.covers_common(&current.opened.common)
            || !F::check_opened(
                conn,
                &current.principal,
                &earlier.input,
                &earlier.value,
                &current.opened.value,
            )
            .await
            .map_err(source_error)?
        {
            return Err(refused());
        }
        Ok(())
    })
}
async fn prepare<I: CommonInitial, F: Source<Role = I::Source>>(
    ctx: &Ctx,
    conn: &mut Conn,
    original: &CapturedCaller,
    input: Arc<F::Input>,
    earlier: Option<&Opened<I, F>>,
) -> ApiResult<(Frame<I, F>, Deadline)> {
    let (first, _) = load::<I, F>(ctx, conn, original, Arc::clone(&input)).await?;
    if let Some(earlier) = earlier {
        same(conn, &first, earlier).await?;
    }
    F::locks(conn, &first.principal, &input, &first.opened.value)
        .await
        .map_err(source_error)?;
    let (fresh, deadline) = load::<I, F>(ctx, conn, &first.opened.caller, input).await?;
    same(conn, &fresh, &first.opened).await?;
    fresh.check_deadline(ctx.app.clock.as_ref())?;
    deadline.check()?;
    Ok((fresh, deadline))
}

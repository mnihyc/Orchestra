//! Native staged readers: owned source operands first, then one non-retried IO start.

use super::*;
use std::future::Future;

/// The exact successful source frame. This is not a lease over later model work
/// or future hits: every later source/delivery needs fresh factual admission.
pub(crate) struct Frame<T> {
    pub ctx: Ctx,
    pub accepted: Caller,
    pub value: T,
    identity: CurrentHttpIdentity,
}

impl<T> Frame<T> {
    fn check_deadline(&self) -> ApiResult<()> {
        self.identity.check_deadline(&self.ctx.app)?;
        if !self
            .accepted
            .key_deadline_allows(self.ctx.app.clock.as_ref())
        {
            return Err(opened_source_refused());
        }
        Ok(())
    }

    /// Move the actual prepared source and its origin into the native owned start.
    /// Capturing this caller preserves its exact accepted operation and all key ceilings.
    pub(crate) fn into_parts(self) -> (Ctx, Caller, T) {
        (self.ctx, self.accepted, self.value)
    }
}

/// Prepare one declared native source inside its own watched snapshot, commit,
/// release the connection, then call the owned start exactly once. No renderer,
/// model work, cursor consumption or supplier IO executes in the retry closure.
///
/// The producer supplies every real role for this stage, including genuine
/// exhaustion. Initial interest never claims an unproduced result set is empty
/// and never substitutes for each later batch's governors. The explicit typed
/// query means later source frames do not replay an original consumed cursor.
pub(crate) fn read<F, Start, Started, Output>(
    ctx: Ctx,
    query: F::Query,
    start: Start,
) -> BoxFuture<'static, ApiResult<Output>>
where
    F: Selected,
    Start: FnOnce(Frame<F::Value>) -> Started + Send + 'static,
    Started: Future<Output = ApiResult<Output>> + Send + 'static,
    Output: Send + 'static,
{
    Box::pin(async move {
        let frame = super::super::snapshot_read::run(ctx, move |ctx| {
            let query = query.clone();
            async move { Box::pin(prepare::<F>(ctx, query)).await }
        })
        .await?;
        frame.check_deadline()?;
        Box::pin(start(frame)).await
    })
}

async fn prepare<F: Selected>(ctx: Ctx, query: F::Query) -> ApiResult<Frame<F::Value>> {
    let ctx = ctx.for_read_preparation();
    let accepted_selection = ctx
        .admitted_caller()
        .and_then(Caller::admitted_operation)
        .map(str::to_owned);
    if !ctx.snapshot_read() || transaction_guard::changes(&ctx) {
        return Err(ApiError::internal(
            "staged native reads require their declared snapshot boundary",
        ));
    }
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let identity =
        transaction_guard::current_http_identity(&ctx, &mut tx, HttpIdentityMode::Read).await?;
    transaction_guard::guard_user(&identity.user)?;
    let mut current = ctx.for_operation_checkpoint(identity.user.clone(), identity.now.get());
    // Ordinary/legacy callers keep the native early door before selection.
    if !modern(&identity.caller) {
        let principal = authorization::load_principal(
            &mut tx,
            identity.caller.clone(),
            current.app.clock.time(),
        )
        .await?;
        let principal =
            crate::sign_in::second_step::hold(&mut tx, &identity.user, principal).await?;
        F::require(&current, &mut tx, &principal).await?;
    }
    let (value, facts) = F::select(&current, &mut tx, &identity.caller, &query).await?;
    let plan = bind(&current, F::declaration(&query, &value), &facts)?;
    if accepted_selection
        .as_deref()
        .is_some_and(|selection| selection != plan.variant().operation)
    {
        return Err(opened_source_refused());
    }
    let plan = match F::origin(&query, &value) {
        Some(origin) => {
            plan.with_origin(&current, &mut tx, &identity.caller, origin)
                .await?
        }
        None => plan,
    };
    if plan
        .shared()
        .required_facets()
        .map_err(admission::binding_error)?
        .iter()
        .any(|facet| facet.action == orch_access_contract::key_scope::KeyAction::Write)
    {
        return Err(ApiError::internal(
            "changing native work requires its effect boundary",
        ));
    }
    let preliminary = plan
        .preparation_principal(&current, &mut tx, &identity.caller)
        .await?;
    F::require(&current, &mut tx, &preliminary).await?;
    let admission = plan.admit(&identity.caller)?;
    if !admission.content {
        return Err(opened_source_refused());
    }
    let accepted = admission.caller.clone();
    current = admission::install(current, admission)?;
    let principal =
        authorization::load_principal(&mut tx, accepted.clone(), current.app.clock.time()).await?;
    let principal = crate::sign_in::second_step::hold(&mut tx, &identity.user, principal).await?;
    F::require(&current, &mut tx, &principal).await?;
    let guard =
        transaction_guard::finish_read_guard(&current, &mut tx, &identity, principal).await?;
    guard.check_deadline(&current)?;
    tx.commit().await.map_err(DbError::from)?;
    let frame = Frame {
        ctx: current,
        accepted,
        value,
        identity,
    };
    frame.check_deadline()?;
    complete_publication(&frame.ctx)?;
    transaction_guard::checked_snapshot_read();
    Ok(frame)
}

//! One bounded native body stage between known destination and complete effect preparation.

use super::*;
use axum::body::Body;

/// Native raw-body decoding and the same existing E producer used after decoding.
/// A known prefix is not complete mutation admission or a deferred-work origin.
pub(crate) trait Staged: effects::Effect<Body = Arc<Self::Input>> {
    type Known: Send + Sync + 'static;
    type Input: Send + Sync + 'static;

    /// Actual header/native intent chooses the canonical variant before consuming bytes.
    fn known_declaration(known: &Self::Known) -> &'static Declaration<Self::Source>;
    fn known<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Self::Known, Vec<Facts<Self::Source>>)>>;
    fn require_known<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        known: &'a Self::Known,
    ) -> BoxFuture<'a, ApiResult<()>>;

    /// Consume the native bounded body once, retaining its actual typed parsed operands.
    /// New unreferenced bytes may use the existing physical staging store. Existing blob
    /// content/size/sniff IO, custody/DB writes and business effects wait for full preparation.
    /// This callback receives no connection and is never stored in the guarded resolver.
    fn take<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        known: Self::Known,
        body: Body,
    ) -> BoxFuture<'a, ApiResult<Self::Input>>;
}

/// The Registry owns this future and its one Body. Final guards retain only immutable input.
pub(crate) async fn take<F: Staged>(
    ctx: Ctx,
    query: F::Query,
    body: Body,
    raw_query: Option<&str>,
) -> ApiResult<(Ctx, Arc<F::Input>)>
where
    F::Query: serde::de::DeserializeOwned,
{
    let original = authorization::caller_of(ctx.user()?)?;
    let (known, plan, accepted, producing) = {
        let mut tx = ctx.app.db.begin_snapshot().await?;
        let identity =
            transaction_guard::current_http_identity(&ctx, &mut tx, HttpIdentityMode::Read).await?;
        transaction_guard::guard_user(&identity.user)?;
        let caller = identity
            .caller
            .intersect_restrictions(&original)
            .map_err(ApiError::internal)?;
        let producing = ctx.for_operation_checkpoint(identity.user.clone(), identity.now.get());
        let current = producing.for_effect_discovery(identity.user.clone(), identity.now.get());
        let (known, facts) = F::known(&current, &mut tx, &caller, query).await?;
        let plan = bind_known(&current, F::known_declaration(&known), &facts)?;
        if !plan.shared().covers_known_required_sources(plan.shared()) {
            return Err(ApiError::internal(
                "a raw body stage needs a real known required destination",
            ));
        }
        let principal = preparation_principal(&current, &mut tx, &caller, &plan).await?;
        F::require_known(&current, &mut tx, &principal, &known).await?;
        // Keep only credential restrictions. No partial CapturedOperation is created.
        let accepted = plan
            .shared()
            .preparation_caller(principal.caller())
            .map_err(admission::binding_error)?;
        tx.commit().await.map_err(DbError::from)?;
        identity.check_deadline(&ctx.app)?;
        if !accepted.key_deadline_allows(ctx.app.clock.as_ref()) {
            return Err(ended());
        }
        (known, plan, accepted, producing)
    };
    let mut ctx = producing.with_operation_ceiling(accepted.clone())?;
    let input = Arc::new(F::take(&ctx, &accepted, known, body).await?);
    if !accepted.key_deadline_allows(ctx.app.clock.as_ref()) {
        return Err(ended());
    }
    let (current, user, now) = {
        let mut conn = ctx.app.db.acquire().await?;
        let identity =
            transaction_guard::current_http_identity(&ctx, &mut conn, HttpIdentityMode::Read)
                .await?;
        let current = crate::registry::restore_operation_caller(&ctx, &mut conn, &accepted).await?;
        identity.check_deadline(&ctx.app)?;
        (current, identity.user, identity.now)
    };
    ctx = ctx.for_operation_checkpoint(user, now.get());
    ctx = ctx.with_operation_ceiling(current.clone())?;
    let request = resolver::OperationRequest::raw_owned(
        ctx.route,
        effects::prepare::<F>,
        raw_query,
        Arc::clone(&input),
        plan.shared().clone(),
        &current,
    )?;
    ctx = ctx.with_operation_request(request);
    let ctx = crate::registry::prepare_registered_raw_body::<F::Query>(ctx, raw_query).await?;
    Ok((ctx, input))
}

fn ended() -> ApiError {
    ApiError::new(
        orch_api_contract::common::ErrorCode::AuthInvalidToken,
        "the raw input credential has ended",
    )
}

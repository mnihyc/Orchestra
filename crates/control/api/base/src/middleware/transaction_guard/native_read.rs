//! Fresh identity for the private native reader, without a raw HTTP credential claim.

use super::*;
pub(super) async fn current(
    ctx: &Ctx,
    conn: &mut Conn,
    mode: HttpIdentityMode,
) -> ApiResult<Option<CurrentHttpIdentity>> {
    let original = ctx.authenticated_user()?;
    let Via::OwnerRead { source } = &original.via else {
        return Ok(None);
    };
    if ctx.surface.is_some()
        || !(ctx.snapshot_read() || ctx.route.method == orch_api_contract::routes::Method::GET)
        || !ctx
            .ask_read
            .as_ref()
            .map(|read| read.allows_read_route(&ctx.route))
            .transpose()?
            .unwrap_or(false)
        || !rows::in_transaction(conn)
        || ctx
            .ask_read
            .as_ref()
            .and_then(|read| read.native_identity())
            != Some(source)
    {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "This native source permits its admitted catalogue reads only",
        ));
    }
    // A read adapter may use its ordinary guard in Action mode for an effect-free GET.
    // Its selected operation still passes the derived Read-only footprint before rendering.
    if matches!(mode, HttpIdentityMode::Action) && changes(ctx) {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "A native owner read cannot perform an action",
        ));
    }
    let requested = ctx.user()?;
    if original.id() != requested.id()
        || original.as_admin
        || requested.as_admin
        || !matches!(&requested.via, Via::OwnerRead {source: current} if current == source)
    {
        return Err(ApiError::internal(
            "the native read changed its original identity",
        ));
    }
    let user = source.current(conn, &ctx.app).await?;
    guard_user(&user)?;
    let mut caller = authorization::caller_of(&user)?;
    // The short live-source scope is required even for the first opening.
    caller = orch_authz::CapturedCaller::capture(&caller)
        .restore_current(conn, ctx.app.clock.as_ref())
        .await
        .map_err(authorization::restoration_error)?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "The original native read source is no longer admitted",
            )
        })?;
    for accepted in ctx
        .operation_ceiling()
        .into_iter()
        .chain(ctx.admitted_caller())
    {
        if !authorization::same_credential_origin(&user, accepted) {
            return Err(ApiError::internal(
                "the native read changed its accepted source",
            ));
        }
        caller = caller
            .intersect_restrictions(accepted)
            .map_err(ApiError::internal)?;
    }
    if let Some(accepted) = ctx.publication.retained_ceiling()? {
        let accepted = accepted
            .restore_current(conn, ctx.app.clock.as_ref())
            .await
            .map_err(authorization::restoration_error)?
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::AuthzDenied,
                    "The original native read source is no longer admitted",
                )
            })?;
        caller = caller
            .intersect_restrictions(&accepted)
            .map_err(ApiError::internal)?;
    }
    ctx.publication.retain_ceiling(&caller)?;
    Ok(Some(CurrentHttpIdentity {
        user,
        caller,
        now: ctx.app.clock.time(),
        expires_at: None,
    }))
}

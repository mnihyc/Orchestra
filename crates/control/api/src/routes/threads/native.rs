//! Current thread viewers read an existing native session's effective commands with a fresh return guard.

use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use orch_access_contract::permissions::Permission;
use orch_api_contract::native::{
    NativeCatalog, ThreadNativeActionList, ThreadNativeActionListQuery,
};
use orch_authz::{Act, Policy};

/// Binds the existing-session catalogue reader.
pub fn mount(registry: &mut Registry) {
    registry.on(ThreadNativeActionList, list);
}

async fn list(ctx: Ctx, query: ThreadNativeActionListQuery, (): ()) -> ApiResult<NativeCatalog> {
    let thread = ctx.id("id")?;
    let catalog = ctx
        .app
        .engine
        .native_actions(thread, query.refresh.unwrap_or(false))
        .await
        .map_err(ApiError::internal)?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let permitted = orch_authz::nodes::check(
        &mut conn,
        &principal,
        thread,
        Permission::ThreadView,
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    if !permitted.is_allowed() {
        return Err(ApiError::not_found());
    }
    Ok(catalog)
}

/// Refreshes only after the caller's ordinary input authority, before its guarded acceptance.
pub(crate) async fn refresh_for_input(ctx: &Ctx, thread: orch_types::Id) -> ApiResult<()> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(ctx, &mut conn).await?;
    let decision = orch_authz::nodes::check(
        &mut conn,
        &principal,
        thread,
        Permission::ThreadInteract,
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    if let orch_access_contract::decisions::Decision::Refused(refusal) = decision {
        return Err(crate::middleware::authorization::refusals::denied(&mut conn, refusal).await?);
    }
    drop(conn);
    ctx.app
        .engine
        .native_actions(thread, true)
        .await
        .map_err(ApiError::internal)?;
    Ok(())
}

//! Authorized recovery reads and explicit retry-now or stop controls for a thread.

use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use orch_api_contract::calls::{
    ThreadCallRecoveryGet, ThreadCallRecoveryReply, ThreadCallRecoveryUpdate,
    ThreadCallRecoveryUpdateBody,
};

/// Binds recovery requests to their contract declarations.
pub fn mount(registry: &mut Registry) {
    registry
        .on_with_operation(
            ThreadCallRecoveryGet,
            crate::key_operations::direct::declared,
            read,
        )
        .on(ThreadCallRecoveryUpdate, update);
}

async fn read(ctx: Ctx, (): (), (): ()) -> ApiResult<ThreadCallRecoveryReply> {
    let node = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    authorization::require_on_path_in(&ctx, &mut conn, &principal, "thread.view").await?;
    Ok(ThreadCallRecoveryReply {
        recovery: orch_engine::calls::view_at(&mut conn, node, ctx.app.clock.now())
            .await
            .map_err(ApiError::engine)?,
    })
}

async fn update(
    ctx: Ctx,
    (): (),
    body: ThreadCallRecoveryUpdateBody,
) -> ApiResult<ThreadCallRecoveryReply> {
    let node = ctx.id("id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    if let Err(error) = ctx
        .app
        .engine
        .control_call_recovery(tx, guard.into_principal(), node, body.action)
        .await
    {
        let mut conn = ctx.app.db.acquire().await?;
        return Err(super::input_failed(&mut conn, node, error).await);
    }
    let mut conn = ctx.app.db.acquire().await?;
    Ok(ThreadCallRecoveryReply {
        recovery: orch_engine::calls::view_at(&mut conn, node, ctx.app.clock.now())
            .await
            .map_err(ApiError::engine)?,
    })
}

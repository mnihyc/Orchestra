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
        .on(ThreadCallRecoveryGet, read)
        .on(ThreadCallRecoveryUpdate, update);
}

async fn read(ctx: Ctx, (): (), (): ()) -> ApiResult<ThreadCallRecoveryReply> {
    let node = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    authorization::principal(&ctx, &mut conn).await?;
    Ok(ThreadCallRecoveryReply {
        recovery: orch_engine::calls::view(&mut conn, node)
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
        recovery: orch_engine::calls::view(&mut conn, node)
            .await
            .map_err(ApiError::engine)?,
    })
}

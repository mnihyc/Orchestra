//! What fills a thread's context, what its turns used beyond their totals, and what a turn was counted at: read as the engine keeps them ([`orch_engine::context_fill`]), and a count asked of the harness on request.
//!
//! Reading takes what reading the thread takes, which the access engine decides before a handler runs. A
//! count (`detail=count`) asks the harness of an idle thread's open session, through the live command route;
//! the engine keeps what it answers with the latest turn, once for each turn, and it changes nothing else:
//! the harness counts with its vendor's token counter, which is not billed, and no turn is started.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use orch_api_contract::context::{
    ContextDetail, TURN_USAGE_PAGE_DEFAULT, TURN_USAGE_PAGE_MAX, ThreadContext, ThreadContextGet,
    ThreadContextGetQuery, ThreadTurnCostGet, ThreadTurnUsage, ThreadTurnUsageList,
    ThreadTurnUsageListQuery, TurnCost,
};
use orch_engine::context_fill;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on(ThreadContextGet, get);
    registry.on(ThreadTurnUsageList, turns);
    registry.on(ThreadTurnCostGet, cost);
}

/// What fills the context, as kept; with `detail=count`, counted by the harness first where it can be.
async fn get(ctx: Ctx, query: ThreadContextGetQuery, (): ()) -> ApiResult<ThreadContext> {
    let thread = ctx.id("id")?;
    let kept = {
        let mut conn = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
        let kept = context_fill::thread_context(&mut conn, thread)
            .await
            .map_err(ApiError::internal)?;
        conn.commit().await.map_err(orch_db::DbError::from)?;
        kept
    };
    if query.detail != Some(ContextDetail::Count) || !kept.can_count {
        return Ok(kept);
    }
    // A harness that cannot count now answers nothing, and what was kept stands.
    let counted = ctx
        .app
        .engine
        .count_context(thread)
        .await
        .map_err(ApiError::internal)?;
    if counted.is_none() {
        return Ok(kept);
    }
    let mut conn = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let kept = context_fill::thread_context(&mut conn, thread)
        .await
        .map_err(ApiError::internal)?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(kept)
}

/// A page of the thread's turns with what each used, the latest last.
async fn turns(ctx: Ctx, query: ThreadTurnUsageListQuery, (): ()) -> ApiResult<ThreadTurnUsage> {
    let thread = ctx.id("id")?;
    let limit = match query.limit {
        None => TURN_USAGE_PAGE_DEFAULT,
        Some(0) => return Err(ApiError::invalid("limit", "must be at least 1")),
        Some(limit) => limit.min(TURN_USAGE_PAGE_MAX),
    };
    let mut conn = ctx.app.db.acquire().await?;
    context_fill::turn_usage_page(&mut conn, thread, query.before, limit)
        .await
        .map_err(ApiError::internal)
}

/// What one turn was counted at, by model.
async fn cost(ctx: Ctx, (): (), (): ()) -> ApiResult<TurnCost> {
    let thread = ctx.id("id")?;
    let turn = ctx.id("turn_id")?;
    let mut conn = ctx.app.db.acquire().await?;
    context_fill::turn_cost(&mut conn, thread, turn)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)
}

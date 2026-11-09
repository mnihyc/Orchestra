//! The raw events of a segment of a thread, for whoever manages the thread: what went to the harness and what came back, newest first.
//!
//! The engine keeps what an adapter reports of its harness's own protocol, masked, for as long as
//! the system's retention says ([`orch_engine::raw_events`]). Who may read it is the thread's
//! managers, which the access engine decides before the handler runs; the segment has to be one of
//! the thread's, or it is not there.

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use orch_api_contract::threads::ThreadRawList;

use orch_api_contract::threads::ThreadRawListQuery;

use orch_api_contract::threads::ThreadRawListReply;

use orch_db::queries::threads as rows;

use orch_engine::raw_events;

use orch_engine::raw_events::PAGE_DEFAULT;

use orch_engine::raw_events::PAGE_MAX;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on_with_operation(ThreadRawList, crate::key_operations::direct::declared, list);
}

/// A page of the raw events of one segment, newest first, from the number given backwards.
async fn list(ctx: Ctx, query: ThreadRawListQuery, (): ()) -> ApiResult<ThreadRawListReply> {
    let thread = ctx.id("id")?;
    let limit = match query.limit {
        None => PAGE_DEFAULT,
        Some(0) => return Err(ApiError::invalid("limit", "must be at least 1")),
        Some(limit) => limit.min(PAGE_MAX),
    };
    if query.before.is_some_and(|before| before < 1) {
        return Err(ApiError::invalid(
            "before",
            "is the number of an event, from 1",
        ));
    }
    let mut conn = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    authorization::require_on_path_in(&ctx, &mut conn, &principal, "thread.manage").await?;
    let projection = orch_db::queries::thread_continuations::projection(
        &mut conn,
        thread,
        query.continuation_id,
    )
    .await?
    .ok_or_else(ApiError::not_found)?;
    if !projection.accepts_cursor(query.before.is_some(), query.projection_revision) {
        return Err(super::read_failed(
            orch_engine::views::ReadError::ProjectionChanged(
                orch_engine::continuations::projection::scope(&projection),
            ),
        ));
    }
    let segment = rows::segment(&mut conn, query.segment_id)
        .await?
        .filter(|segment| segment.thread_id == thread)
        .ok_or_else(ApiError::not_found)?;
    if orch_db::queries::thread_continuations::reading::segments_among(
        &mut conn,
        thread,
        projection.viewed_continuation_id,
        &[segment.id],
    )
    .await?
    .is_empty()
    {
        return Err(ApiError::not_found());
    }
    if !orch_db::queries::thread_history::complete_segment(
        &mut conn,
        thread,
        projection.viewed_continuation_id,
        segment.id,
    )
    .await?
    {
        return Err(ApiError::new(orch_api_contract::common::ErrorCode::RequestValueRefused,
            "These raw protocol frames describe the whole native session. Open its original history to read them.")
            .with("continuation_id",segment.continuation_id));
    }
    let row = rows::thread(&mut conn, thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let page = raw_events::page(&mut conn, &row, segment.id, query.before, limit)
        .await
        .map_err(ApiError::internal)?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    // Preserve the existing post-snapshot checkpoint, with current target facts as well.
    let mut current = ctx.app.db.acquire().await?;
    let principal = ctx.operation_principal(&mut current).await?;
    authorization::require_on_path_in(&ctx, &mut current, &principal, "thread.manage").await?;
    Ok(ThreadRawListReply {
        projection: Some(orch_engine::continuations::projection::scope(&projection)),
        events: page.events,
        has_older: page.has_older,
    })
}

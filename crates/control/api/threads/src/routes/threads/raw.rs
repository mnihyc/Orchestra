//! The raw events of a segment of a thread, for whoever manages the thread: what went to the harness and what came back, newest first.
//!
//! The engine keeps what an adapter reports of its harness's own protocol, masked, for as long as
//! the system's retention says ([`orch_engine::raw_events`]). Who may read it is the thread's
//! managers, which the access engine decides before the handler runs; the segment has to be one of
//! the thread's, or it is not there.

use crate::key_operations::snapshot_read;

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

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on_with_operation(ThreadRawList, crate::key_operations::direct::declared, list);
}

/// A page of the raw events of one segment, newest first, from the number given backwards.
async fn list(ctx: Ctx, query: ThreadRawListQuery, (): ()) -> ApiResult<ThreadRawListReply> {
    snapshot_read::run(ctx, move |ctx| list_snapshot(ctx, query.clone())).await
}

async fn list_snapshot(ctx: Ctx, query: ThreadRawListQuery) -> ApiResult<ThreadRawListReply> {
    let thread = ctx.id("id")?;
    let limit = match query.limit {
        None => PAGE_DEFAULT,
        Some(0) => return Err(ApiError::invalid("limit", "must be at least 1")),
        Some(limit) => limit,
    };
    if query.before.is_some_and(|before| before < 1) {
        return Err(ApiError::invalid(
            "before",
            "is the number of an event, from 1",
        ));
    }
    let mut conn = orch_db::queries::raw_events::begin_reading(&ctx.app.db).await?;
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
    let words = orch_db::queries::search::Query::parse(query.q.as_deref().unwrap_or(""));
    let filtering = !words.terms().is_empty() || query.direction.is_some();
    let batch = if filtering {
        limit.max(PAGE_DEFAULT)
    } else {
        limit
    };
    let mut batches = if filtering {
        super::read_history::search_batches(&mut conn).await?
    } else {
        1
    };
    let mut before = query.before;
    let mut events = Vec::new();
    let next_before = loop {
        let page = raw_events::page(&mut conn, &row, segment.id, before, batch)
            .await
            .map_err(snapshot_read::engine_error)?;
        let next = page
            .has_older
            .then(|| page.events.last().map(|event| event.n))
            .flatten();
        events.extend(page.events.into_iter().filter(|event| {
            query
                .direction
                .is_none_or(|direction| direction == event.direction)
                && words.matches(&event.payload.to_string())
        }));
        let extra = events.len() > usize::try_from(limit).map_err(ApiError::internal)?;
        events.truncate(usize::try_from(limit).map_err(ApiError::internal)?);
        batches = batches.saturating_sub(1);
        if extra {
            break events.last().map(|event| event.n);
        }
        if next.is_none()
            || batches == 0
            || events.len() == usize::try_from(limit).map_err(ApiError::internal)?
        {
            break next;
        }
        if next == before {
            return Err(ApiError::internal(
                "the raw search repeated its native position",
            ));
        }
        before = next;
    };
    let capture = orch_db::queries::raw_events::capture(&mut conn, segment.id).await?;
    let retention = raw_events::retention(&mut conn, principal.now().get())
        .await
        .map_err(snapshot_read::engine_error)?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    // Preserve the existing post-snapshot checkpoint, with current target facts as well.
    let mut current = ctx.app.db.acquire().await?;
    let principal = ctx.operation_principal(&mut current).await?;
    authorization::require_on_path_in(&ctx, &mut current, &principal, "thread.manage").await?;
    Ok(ThreadRawListReply {
        projection: Some(orch_engine::continuations::projection::scope(&projection)),
        events,
        has_older: next_before.is_some(),
        next_before,
        capture,
        retention: Some(retention),
    })
}

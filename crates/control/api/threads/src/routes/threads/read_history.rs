//! Explicit historical turn and segment pages, projected with only their required references.

use super::read_operation::prepare;

use super::read_operation::require_current;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use orch_api_contract::threads::ITEMS_DEFAULT_LIMIT;

use orch_api_contract::threads::ITEMS_MAX_LIMIT;

use orch_api_contract::threads::ThreadHistoryListQuery;

use orch_api_contract::threads::ThreadSegmentList;

use orch_api_contract::threads::ThreadSegmentPage;

use orch_api_contract::threads::ThreadTurnList;

use orch_api_contract::threads::ThreadTurnPage;

use orch_db::queries::feeds;

use orch_db::queries::thread_continuations as continuation_rows;

use orch_db::queries::threads;
pub(super) fn mount(registry: &mut Registry) {
    registry
        .on_with_operation(ThreadSegmentList, prepare, segments)
        .on_with_operation(ThreadTurnList, prepare, turns);
}

fn limit(query: &ThreadHistoryListQuery) -> ApiResult<i64> {
    if query.before.is_some_and(|before| before <= 0) {
        return Err(ApiError::invalid(
            "before",
            "is a positive permanent ordinal",
        ));
    }
    Ok(i64::from(super::read::limit_of(
        query.limit,
        ITEMS_DEFAULT_LIMIT,
        ITEMS_MAX_LIMIT,
    )?))
}

async fn segments(ctx: Ctx, query: ThreadHistoryListQuery, (): ()) -> ApiResult<ThreadSegmentPage> {
    let limit = limit(&query)?;
    let thread = ctx.id("id")?;
    let mut tx = feeds::begin_reading(&ctx.app.db).await?;
    require_current(&ctx, &mut tx).await?;
    let head = threads::thread(&mut tx, thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let projection = continuation_rows::projection(&mut tx, thread, query.continuation_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !projection.accepts_cursor(query.before.is_some(), query.projection_revision) {
        return Err(super::read_failed(
            orch_engine::views::ReadError::ProjectionChanged(
                orch_engine::continuations::projection::scope(&projection),
            ),
        ));
    }
    let mut rows = continuation_rows::reading::segments(
        &mut tx,
        thread,
        projection.viewed_continuation_id,
        query.before,
        limit + 1,
    )
    .await?;
    let more = i64::try_from(rows.len()).unwrap_or(i64::MAX) > limit;
    if more {
        rows.remove(0);
    }
    let next_before = if more {
        rows.first().map(|row| row.ordinal)
    } else {
        None
    };
    let (_, segments) = ctx
        .app
        .engine
        .project_history_in(&mut tx, &[], &rows)
        .await
        .map_err(super::read_failed)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    #[cfg(any(test, feature = "test-support"))]
    crate::realtime::history_read_tests::pause(
        crate::realtime::history_read_tests::Cut::HttpHistoryRows,
    )
    .await;
    let mut current = ctx.app.db.acquire().await?;
    require_current(&ctx, &mut current).await?;
    Ok(ThreadSegmentPage {
        projection: Some(orch_engine::continuations::projection::scope(&projection)),
        segments,
        next_before,
        seq: head.seq(),
    })
}

async fn turns(ctx: Ctx, query: ThreadHistoryListQuery, (): ()) -> ApiResult<ThreadTurnPage> {
    let limit = limit(&query)?;
    let thread = ctx.id("id")?;
    let mut tx = feeds::begin_reading(&ctx.app.db).await?;
    require_current(&ctx, &mut tx).await?;
    let head = threads::thread(&mut tx, thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let projection = continuation_rows::projection(&mut tx, thread, query.continuation_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !projection.accepts_cursor(query.before.is_some(), query.projection_revision) {
        return Err(super::read_failed(
            orch_engine::views::ReadError::ProjectionChanged(
                orch_engine::continuations::projection::scope(&projection),
            ),
        ));
    }
    let mut rows = continuation_rows::reading::turns(
        &mut tx,
        thread,
        projection.viewed_continuation_id,
        query.before,
        limit + 1,
    )
    .await?;
    let more = i64::try_from(rows.len()).unwrap_or(i64::MAX) > limit;
    if more {
        rows.remove(0);
    }
    let next_before = if more {
        rows.first().map(|row| row.ordinal)
    } else {
        None
    };
    let segment_ids: Vec<_> = rows.iter().map(|row| row.segment_id).collect();
    let segments = continuation_rows::reading::segments_among(
        &mut tx,
        thread,
        projection.viewed_continuation_id,
        &segment_ids,
    )
    .await?;
    let (turns, segments) = ctx
        .app
        .engine
        .project_history_in(&mut tx, &rows, &segments)
        .await
        .map_err(super::read_failed)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    #[cfg(any(test, feature = "test-support"))]
    crate::realtime::history_read_tests::pause(
        crate::realtime::history_read_tests::Cut::HttpHistoryRows,
    )
    .await;
    let mut current = ctx.app.db.acquire().await?;
    require_current(&ctx, &mut current).await?;
    Ok(ThreadTurnPage {
        projection: Some(orch_engine::continuations::projection::scope(&projection)),
        turns,
        segments,
        next_before,
        seq: head.seq(),
    })
}

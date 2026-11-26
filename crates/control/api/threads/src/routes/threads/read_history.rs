//! Explicit historical turn and segment pages, projected with only their required references.

use super::read_operation::prepare;

use super::read_operation::require_current;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use orch_api_contract::threads::ITEMS_DEFAULT_LIMIT;

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
    positive_limit(query.limit, ITEMS_DEFAULT_LIMIT)
}

pub(super) fn positive_limit(limit: Option<u32>, default: u32) -> ApiResult<i64> {
    match limit.unwrap_or(default) {
        0 => Err(ApiError::invalid("limit", "must be greater than zero")),
        limit => Ok(i64::from(limit)),
    }
}

pub(super) async fn search_batches(conn: &mut orch_db::Conn) -> ApiResult<u64> {
    orch_settings::get::<u64>(
        conn,
        orch_settings::Place::System,
        orch_settings_contract::registry::keys::SEARCH_HISTORY_CANDIDATE_BATCHES,
    )
    .await
    .map_err(ApiError::internal)?
    .ok_or_else(|| ApiError::internal("the history search work setting has no value"))
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
    let words = orch_db::queries::search::Query::parse(query.q.as_deref().unwrap_or(""));
    let mut before = query.before;
    let scanning = !words.terms().is_empty();
    let batch = if scanning {
        limit.max(i64::from(ITEMS_DEFAULT_LIMIT))
    } else {
        limit
    };
    let mut batches = if scanning {
        search_batches(&mut tx).await?
    } else {
        1
    };
    let mut result = Vec::new();
    let next_before = loop {
        let mut rows = continuation_rows::reading::segments(
            &mut tx,
            thread,
            projection.viewed_continuation_id,
            before,
            batch + 1,
        )
        .await?;
        let more = i64::try_from(rows.len()).unwrap_or(i64::MAX) > batch;
        if more {
            rows.remove(0);
        }
        let next = more.then(|| rows.first().map(|row| row.ordinal)).flatten();
        let (_, projected) = ctx
            .app
            .engine
            .project_history_in(&mut tx, &[], &rows)
            .await
            .map_err(super::read_failed)?;
        result.extend(projected.into_iter().rev().filter(|one| {
            words.matches(&format!(
                "{} · {} · {}",
                one.ordinal,
                one.agent.as_ref().map(|a| a.name.as_str()).unwrap_or(""),
                one.provider.as_ref().map(|p| p.name.as_str()).unwrap_or("")
            ))
        }));
        let extra = i64::try_from(result.len()).unwrap_or(i64::MAX) > limit;
        result.truncate(usize::try_from(limit).map_err(ApiError::internal)?);
        batches = batches.saturating_sub(1);
        if extra {
            break result
                .last()
                .and_then(|row| i32::try_from(row.ordinal).ok());
        }
        if next.is_none()
            || batches == 0
            || i64::try_from(result.len()).unwrap_or(i64::MAX) == limit
        {
            break next;
        }
        before = next;
    };
    result.reverse();
    let segments = result;
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
    let words = orch_db::queries::search::Query::parse(query.q.as_deref().unwrap_or(""));
    let mut before = query.before;
    let scanning = !words.terms().is_empty();
    let batch = if scanning {
        limit.max(i64::from(ITEMS_DEFAULT_LIMIT))
    } else {
        limit
    };
    let mut batches = if scanning {
        search_batches(&mut tx).await?
    } else {
        1
    };
    let mut result = Vec::new();
    let mut related = std::collections::BTreeMap::new();
    let next_before = loop {
        let mut rows = continuation_rows::reading::turns(
            &mut tx,
            thread,
            projection.viewed_continuation_id,
            before,
            batch + 1,
        )
        .await?;
        let more = i64::try_from(rows.len()).unwrap_or(i64::MAX) > batch;
        if more {
            rows.remove(0);
        }
        let next = more.then(|| rows.first().map(|row| row.ordinal)).flatten();
        let segment_ids: Vec<_> = rows.iter().map(|row| row.segment_id).collect();
        let references = continuation_rows::reading::segments_among(
            &mut tx,
            thread,
            projection.viewed_continuation_id,
            &segment_ids,
        )
        .await?;
        let (projected, projected_segments) = ctx
            .app
            .engine
            .project_history_in(&mut tx, &rows, &references)
            .await
            .map_err(super::read_failed)?;
        result.extend(projected.into_iter().rev().filter(|one| {
            words.matches(&format!(
                "{} {} {}",
                one.ordinal,
                one.model,
                serde_json::to_value(one.state).unwrap_or_default()
            ))
        }));
        for segment in projected_segments {
            related.insert(segment.id, segment);
        }
        let extra = i64::try_from(result.len()).unwrap_or(i64::MAX) > limit;
        result.truncate(usize::try_from(limit).map_err(ApiError::internal)?);
        batches = batches.saturating_sub(1);
        if extra {
            break result
                .last()
                .and_then(|row| i32::try_from(row.ordinal).ok());
        }
        if next.is_none()
            || batches == 0
            || i64::try_from(result.len()).unwrap_or(i64::MAX) == limit
        {
            break next;
        }
        before = next;
    };
    result.reverse();
    let mut turns = result;
    let ids = turns
        .iter()
        .map(|turn| turn.segment_id)
        .collect::<std::collections::BTreeSet<_>>();
    let segments = related
        .into_values()
        .filter(|segment| ids.contains(&segment.id))
        .collect();
    orch_engine::notifications::decisions::project(&mut tx, ctx.user()?.id(), &mut turns, &mut [])
        .await
        .map_err(ApiError::internal)?;
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

//! Reading a thread: a sync that brings a client up to date, a stretch of the timeline around a place, one item, a command's output as plain text, the outline, the catch-up, and how far the caller has read.
//!
//! What a reader is given is what the engine holds, turned into the wire's
//! types there; the thread's own view, with what the caller holds on it, is
//! made by [`super::view`] and put beside it. Reading needs `thread.view` on
//! the node in the path; storing a read position checks it again inside the
//! transaction that writes that position.

use super::{one, read_failed};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use axum::body::Body;
use axum::response::Response;
use http::{HeaderValue, StatusCode, header};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::personal::{ThreadRead, ThreadReadBody, ThreadReadReply};
use orch_api_contract::threads::continuations::ThreadReadSelector;
use orch_api_contract::threads::{
    Catchup, CatchupItem, ITEMS_DEFAULT_LIMIT, ITEMS_MAX_LIMIT, Item, OUTLINE_TITLE_CHARS,
    OUTPUT_DEFAULT_LIMIT_BYTES, OutlineEntry, OutlineKind, THREAD_SYNC_DEFAULT_LIMIT,
    THREAD_SYNC_MAX_LIMIT, TOTAL_LENGTH_HEADER, ThreadCatchupGet, ThreadCatchupGetQuery,
    ThreadItemGet, ThreadItemList, ThreadItemListQuery, ThreadItemListReply, ThreadItemOutputGet,
    ThreadItemOutputGetQuery, ThreadOutlineGet, ThreadOutlineGetQuery, ThreadOutlinePage,
    ThreadSync, ThreadSyncGet, ThreadSyncGetQuery,
};
use orch_db::queries::{approvals, thread_continuations as continuation_rows, threads as rows};
use orch_engine::views::{Around, ThreadFrame, search_text};
use orch_types::Time;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(ThreadSyncGet, sync)
        .on(ThreadItemList, item_list)
        .on(ThreadItemGet, item_get)
        .on_raw(ThreadItemOutputGet, output_get)
        .on(ThreadOutlineGet, outline)
        .on(ThreadCatchupGet, catchup)
        .on(ThreadRead, read);
}

/// A limit as asked, within what the request allows.
pub(super) fn limit_of(asked: Option<u32>, default: u32, most: u32) -> ApiResult<u32> {
    match asked {
        None => Ok(default),
        Some(0) => Err(ApiError::invalid("limit", "must be at least 1")),
        Some(limit) => Ok(limit.min(most)),
    }
}

/// A coherent window with its required metadata, or what changed since a sequence.
pub(super) async fn sync(ctx: Ctx, query: ThreadSyncGetQuery, (): ()) -> ApiResult<ThreadSync> {
    let mut tx = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let reply = sync_in(&ctx, &mut tx, query).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    crate::middleware::authorization::require_on_path(&ctx, "thread.view").await?;
    Ok(reply)
}

/// Core conversation rows, native view and request cards share the caller's snapshot.
pub(super) async fn sync_in(
    ctx: &Ctx,
    tx: &mut orch_db::Conn,
    query: ThreadSyncGetQuery,
) -> ApiResult<ThreadSync> {
    let thread = ctx.id("id")?;
    let limit = limit_of(
        query.limit,
        THREAD_SYNC_DEFAULT_LIMIT,
        THREAD_SYNC_MAX_LIMIT,
    )?;
    let view = one(ctx, tx, thread).await?;
    #[cfg(test)]
    crate::realtime::history_read_tests::pause(crate::realtime::history_read_tests::Cut::HttpHead)
        .await;
    let data = ctx
        .app
        .engine
        .sync_view_in(
            tx,
            thread,
            Some(ctx.user()?.id()),
            query.since,
            limit,
            ThreadReadSelector {
                continuation_id: query.continuation_id,
                projection_revision: query.projection_revision,
                ..Default::default()
            },
        )
        .await
        .map_err(read_failed)?;
    let mut reply = data.assemble(ThreadFrame {
        goal: view.goal,
        base: view.base,
        binding: view.binding,
        side_of: view.side_of,
        sides: view.sides,
        memory: view.memory,
        lock: view.lock,
        blocked: view.blocked,
        environment_permissions: view.environment_permissions,
        directory_threads: view.directory_threads,
        input_access: view.input_access,
        can: view.can,
    });
    if reply.complete
        && let Some(projection) = &reply.projection
    {
        let principal = crate::middleware::authorization::principal(ctx, tx).await?;
        let mut ids = orch_engine::continuations::projection::item_inputs(&reply.items);
        let changed = orch_engine::continuations::projection::changed_ids(
            tx,
            thread,
            projection.viewed_continuation_id,
            query.since,
            orch_api_contract::threads::THREAD_SYNC_MAX_CHANGES + 1,
        )
        .await;
        let changed = match changed {
            Ok(changed) => changed,
            Err(error) => return Err(super::continuations::refused_in(tx, thread, error).await),
        };
        if i64::try_from(changed.len()).unwrap_or(i64::MAX)
            > orch_api_contract::threads::THREAD_SYNC_MAX_CHANGES
        {
            reply.complete = false;
            reply.items.clear();
            reply.turns.clear();
            reply.inputs.clear();
            reply.segments.clear();
        } else {
            ids.extend(changed);
            let metadata = match orch_engine::continuations::projection::metadata(
                &ctx.app.engine,
                tx,
                &principal,
                thread,
                projection.viewed_continuation_id,
                ids,
            )
            .await
            {
                Ok(metadata) => metadata,
                Err(error) => return Err(super::continuations::refused_in(tx, thread, error).await),
            };
            reply.prompt_summaries = metadata.summaries;
            reply.viewed_prompt = metadata.viewed_prompt;
        }
    }
    if reply.items.iter().any(|item| {
        matches!(
            &item.payload,
            orch_api_contract::threads::ItemPayload::Approval(_)
        )
    }) {
        let principal = crate::middleware::authorization::principal(ctx, tx).await?;
        super::approval_items::project(
            tx,
            &principal,
            thread,
            &mut reply.items,
            ctx.now,
            &ctx.app.engine,
        )
        .await?;
    }
    Ok(reply)
}

/// Which place of the timeline a read is taken around: exactly one of three.
fn around_of(query: &ThreadItemListQuery) -> ApiResult<Around> {
    match (query.before, query.after, query.around) {
        (Some(seq), None, None) => Ok(Around::Before(seq)),
        (None, Some(seq), None) => Ok(Around::After(seq)),
        (None, None, Some(item)) => Ok(Around::Item(item)),
        _ => Err(ApiError::invalid(
            "",
            "exactly one of `before`, `after` and `around` is given",
        )),
    }
}

/// A stretch of the timeline, oldest first.
async fn item_list(ctx: Ctx, query: ThreadItemListQuery, (): ()) -> ApiResult<ThreadItemListReply> {
    let thread = ctx.id("id")?;
    let mut tx = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let projection = continuation_rows::projection(&mut tx, thread, query.continuation_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let around = if let Some(turn) = query.turn {
        if turn < 1 || query.before.is_some() || query.after.is_some() || query.around.is_some() {
            return Err(ApiError::invalid(
                "turn",
                "choose one positive turn number or another window",
            ));
        }
        let anchor = {
            continuation_rows::reading::first_item_of_turn(
                &mut tx,
                thread,
                projection.viewed_continuation_id,
                turn,
            )
            .await?
            .ok_or_else(ApiError::not_found)?
        };
        Around::After(anchor.1.saturating_sub(1))
    } else {
        around_of(&query)?
    };
    let limit = limit_of(query.limit, ITEMS_DEFAULT_LIMIT, ITEMS_MAX_LIMIT)?;
    let mut page = ctx
        .app
        .engine
        .items_view_in(
            &mut tx,
            thread,
            around,
            limit,
            query.stream.as_deref(),
            ThreadReadSelector {
                continuation_id: query.continuation_id,
                projection_revision: query.projection_revision,
                ..Default::default()
            },
        )
        .await
        .map_err(read_failed)?;
    let principal = crate::middleware::authorization::principal(&ctx, &mut tx).await?;
    let prompt_summaries = match orch_engine::continuations::projection::summaries(
        &ctx.app.engine,
        &mut tx,
        &principal,
        thread,
        page.projection.viewed_continuation_id,
        orch_engine::continuations::projection::item_inputs(&page.items),
    )
    .await
    {
        Ok(summaries) => summaries,
        Err(error) => return Err(super::continuations::refused_in(&mut tx, thread, error).await),
    };
    if page.items.iter().any(|item| {
        matches!(
            &item.payload,
            orch_api_contract::threads::ItemPayload::Approval(_)
        )
    }) {
        let principal = crate::middleware::authorization::principal(&ctx, &mut tx).await?;
        super::approval_items::project(
            &mut tx,
            &principal,
            thread,
            &mut page.items,
            ctx.now,
            &ctx.app.engine,
        )
        .await?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    crate::middleware::authorization::require_on_path(&ctx, "thread.view").await?;
    Ok(ThreadItemListReply {
        projection: Some(page.projection),
        prompt_summaries,
        items: page.items,
        turns: page.turns,
        segments: page.segments,
        has_older: page.has_older,
        has_newer: page.has_newer,
    })
}

/// One item of the thread.
async fn item_get(
    ctx: Ctx,
    query: orch_api_contract::threads::continuations::ThreadReadSelector,
    (): (),
) -> ApiResult<Item> {
    let thread = ctx.id("id")?;
    let item = ctx.id("item_id")?;
    let mut tx = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let mut found = ctx
        .app
        .engine
        .item_view_in(
            &mut tx,
            thread,
            item,
            query.continuation_id,
            query.projection_revision,
        )
        .await
        .map_err(read_failed)?;
    if matches!(
        &found.payload,
        orch_api_contract::threads::ItemPayload::Approval(_)
    ) {
        let principal = crate::middleware::authorization::principal(&ctx, &mut tx).await?;
        super::approval_items::project(
            &mut tx,
            &principal,
            thread,
            std::slice::from_mut(&mut found),
            ctx.now,
            &ctx.app.engine,
        )
        .await?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    crate::middleware::authorization::require_on_path(&ctx, "thread.view").await?;
    Ok(found)
}

/// A piece of a command's output as plain text, with the whole length in a
/// header.
async fn output_get(ctx: Ctx, query: ThreadItemOutputGetQuery, _: Body) -> ApiResult<Response> {
    let thread = ctx.id("id")?;
    let item = ctx.id("item_id")?;
    let offset = query.offset.unwrap_or_default();
    if offset < 0 {
        return Err(ApiError::invalid("offset", "must not be negative"));
    }
    let limit = query.limit.unwrap_or(OUTPUT_DEFAULT_LIMIT_BYTES);
    if limit < 1 {
        return Err(ApiError::invalid("limit", "must be at least 1"));
    }
    let page = ctx
        .app
        .engine
        .command_output_view(
            thread,
            item,
            offset,
            limit,
            query.continuation_id,
            query.projection_revision,
        )
        .await
        .map_err(read_failed)?;
    let mut response = Response::new(Body::from(page.text));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    if let Ok(total) = HeaderValue::from_str(&page.total_len.to_string()) {
        headers.insert(TOTAL_LENGTH_HEADER, total);
    }
    Ok(response)
}

/// Records how far the caller has read; it only ever goes forward.
async fn read(ctx: Ctx, (): (), body: ThreadReadBody) -> ApiResult<ThreadReadReply> {
    let thread = ctx.id("id")?;
    if body.seq < 0 {
        return Err(ApiError::new(
            ErrorCode::RequestInvalid,
            "seq must not be negative",
        ));
    }
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let row = rows::thread(&mut tx, thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let projection = continuation_rows::projection(&mut tx, thread, body.continuation_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !projection.accepts_cursor(true, body.projection_revision) {
        return Err(read_failed(
            orch_engine::views::ReadError::ProjectionChanged(
                orch_engine::continuations::projection::scope(&projection),
            ),
        ));
    }
    let seen = body.seq.clamp(0, row.seq());
    let stored = continuation_rows::mark_read(
        &mut tx,
        guard.user().id(),
        thread,
        projection.viewed_continuation_id,
        seen,
        guard.now().get(),
    )
    .await?;
    let thread_display = orch_engine::views::thread_displays(
        &mut tx,
        &[thread],
        Some(guard.user().id()),
        guard.now(),
    )
    .await
    .map_err(ApiError::internal)?
    .remove(&thread);
    let event = serde_json::to_value(
        orch_realtime_contract::feeds::UserEvent::ContinuationReadState {
            thread_display: None,
            thread_id: thread,
            continuation_id: projection.viewed_continuation_id,
            last_read_seq: stored,
        },
    )
    .map_err(ApiError::internal)?;
    if !orch_db::sequences::notify_user_event(&mut tx, &[guard.user().id()], &event).await? {
        return Err(ApiError::internal(
            "The scoped thread read notice exceeds its fixed event budget.",
        ));
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(ThreadReadReply {
        thread_display,
        projection: Some(orch_engine::continuations::projection::scope(&projection)),
        last_read_seq: stored,
    })
}

/// The first characters of a text that make the title of a line.
fn title_of(text: &str) -> String {
    let first = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    first
        .trim()
        .chars()
        .take(usize::try_from(OUTLINE_TITLE_CHARS).unwrap_or(usize::MAX))
        .collect()
}

/// The saved condition is human-facing; a goal's native instructions never name an outline point.
fn outline_goal_condition(row: &rows::reading::OutlinePoint) -> Option<String> {
    (row.kind == "user_message"
        && row
            .payload
            .get("source")
            .and_then(serde_json::Value::as_str)
            == Some("goal"))
    .then(|| {
        row.payload
            .get("goal_condition")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    })
}

/// The line an item makes in the outline, if its kind makes one.
fn outline_line(
    row: &rows::reading::OutlinePoint,
    approval_titles: &std::collections::HashMap<orch_types::Id, String>,
) -> Option<(OutlineKind, String)> {
    let said = |key: &str| {
        row.payload
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(title_of)
            .unwrap_or_default()
    };
    Some(match row.kind.as_str() {
        "user_message" => (
            OutlineKind::UserMessage,
            outline_goal_condition(row).map_or_else(
                || title_of(&search_text("user_message", &row.payload).unwrap_or_default()),
                |condition| title_of(&condition),
            ),
        ),
        "agent_message" => (OutlineKind::AgentMessage, said("text")),
        "announcement"
            if row
                .payload
                .get("category")
                .and_then(serde_json::Value::as_str)
                == Some("milestone") =>
        {
            (OutlineKind::Milestone, said("title"))
        }
        "approval" => {
            let id = row
                .payload
                .get("approval_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|id| id.parse::<orch_types::Id>().ok());
            let title = id
                .and_then(|id| approval_titles.get(&id))
                .map(|title| title_of(title))
                .unwrap_or_default();
            (OutlineKind::Approval, title)
        }
        "tool_call"
            if row
                .payload
                .pointer("/tool/tool")
                .and_then(serde_json::Value::as_str)
                == Some("announce")
                && row
                    .payload
                    .pointer("/tool/arguments/category")
                    .and_then(serde_json::Value::as_str)
                    == Some("milestone") =>
        {
            let title = row
                .payload
                .pointer("/tool/arguments/title")
                .and_then(serde_json::Value::as_str)
                .map(title_of);
            (OutlineKind::Milestone, title?)
        }
        _ => return None,
    })
}

/// The outline of the thread, oldest first: numbered turns, main-stream messages,
/// milestones and approvals, using their conversation words.
pub(super) async fn outline(
    ctx: Ctx,
    query: ThreadOutlineGetQuery,
    (): (),
) -> ApiResult<ThreadOutlinePage> {
    let mut tx = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let page = outline_in(&ctx, &mut tx, query).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    crate::middleware::authorization::require_on_path(&ctx, "thread.view").await?;
    Ok(page)
}

/// The outline shares the initial screen's conversation snapshot, without another pooled read.
pub(super) async fn outline_in(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    query: ThreadOutlineGetQuery,
) -> ApiResult<ThreadOutlinePage> {
    let thread = ctx.id("id")?;
    let limit = i64::from(limit_of(
        query.limit,
        THREAD_SYNC_DEFAULT_LIMIT,
        THREAD_SYNC_MAX_LIMIT,
    )?);
    let anchors = usize::from(query.before.is_some())
        + usize::from(query.around.is_some())
        + usize::from(query.turn.is_some());
    if anchors > 1
        || query.before.is_some_and(|seq| seq <= 0)
        || query.turn.is_some_and(|turn| turn <= 0)
    {
        return Err(ApiError::invalid(
            "",
            "choose one positive before cursor, item anchor or turn number",
        ));
    }
    let head = rows::thread(conn, thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let projection = continuation_rows::projection(conn, thread, query.continuation_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !projection.accepts_cursor(anchors > 0, query.projection_revision) {
        return Err(read_failed(
            orch_engine::views::ReadError::ProjectionChanged(
                orch_engine::continuations::projection::scope(&projection),
            ),
        ));
    }
    let view = projection.viewed_continuation_id;
    let anchor = if let Some(item) = query.around {
        Some(
            continuation_rows::reading::item(conn, thread, view, item)
                .await?
                .ok_or_else(ApiError::not_found)?
                .seq_created,
        )
    } else if let Some(turn) = query.turn {
        Some(
            continuation_rows::reading::first_item_of_turn(conn, thread, view, turn)
                .await?
                .ok_or_else(ApiError::not_found)?
                .1,
        )
    } else {
        None
    };
    let title_chars = i64::from(OUTLINE_TITLE_CHARS);
    let mut found = if let Some(anchor) = anchor {
        let older = (limit + 1) / 2;
        let mut points = rows::reading::outline_points_view(
            conn,
            thread,
            view,
            Some(anchor.saturating_add(1)),
            false,
            older,
            title_chars,
        )
        .await?;
        points.extend(
            rows::reading::outline_points_view(
                conn,
                thread,
                view,
                Some(anchor),
                true,
                limit - older,
                title_chars,
            )
            .await?,
        );
        points
    } else {
        rows::reading::outline_points_view(
            conn,
            thread,
            view,
            query.before,
            false,
            limit + 1,
            title_chars,
        )
        .await?
    };
    let extra = anchor.is_none() && i64::try_from(found.len()).unwrap_or(i64::MAX) > limit;
    if extra {
        found.remove(0);
    }
    let next_before = if let Some(first) = found.first() {
        let older = extra
            || (anchor.is_some()
                && !rows::reading::outline_points_view(
                    conn,
                    thread,
                    view,
                    Some(first.seq_created),
                    false,
                    1,
                    title_chars,
                )
                .await?
                .is_empty());
        older.then_some(first.seq_created)
    } else {
        None
    };
    let approval_ids: Vec<orch_types::Id> = found
        .iter()
        .filter(|row| row.kind == "approval")
        .filter_map(|row| {
            row.payload
                .get("approval_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|id| id.parse().ok())
        })
        .collect();
    let approval_titles = if approval_ids.is_empty() {
        std::collections::HashMap::new()
    } else {
        approvals::titles_in_thread(conn, thread, &approval_ids)
            .await?
            .into_iter()
            .collect()
    };
    let mut lines = Vec::with_capacity(found.len());
    for row in &found {
        let at = Time::new(row.created_at);
        if row.starts_turn
            && let Some(ordinal) = row.turn_ordinal
        {
            lines.push(OutlineEntry {
                item_id: row.id,
                seq: row.seq_created,
                at,
                title: String::new(),
                kind: OutlineKind::Turn,
                turn_ordinal: Some(ordinal),
                goal_condition: None,
            });
        }
        if let Some((kind, title)) = outline_line(row, &approval_titles) {
            lines.push(OutlineEntry {
                item_id: row.id,
                seq: row.seq_created,
                at,
                title,
                kind,
                turn_ordinal: None,
                goal_condition: outline_goal_condition(row),
            });
        }
    }
    Ok(ThreadOutlinePage {
        projection: Some(orch_engine::continuations::projection::scope(&projection)),
        entries: lines,
        next_before,
        seq: head.seq(),
    })
}

/// What happened in the thread after a sequence, in numbers.
async fn catchup(ctx: Ctx, query: ThreadCatchupGetQuery, (): ()) -> ApiResult<Catchup> {
    let thread = ctx.id("id")?;
    let since = query.since;
    if since < 0 {
        return Err(ApiError::invalid("since", "must not be negative"));
    }
    if query.milestones_after.is_some_and(|cursor| cursor < since)
        || query
            .announcements_after
            .is_some_and(|cursor| cursor < since)
    {
        return Err(ApiError::invalid(
            "after",
            "a title cursor must not precede since",
        ));
    }
    let limit = limit_of(query.limit, ITEMS_DEFAULT_LIMIT, ITEMS_MAX_LIMIT)?;
    let mut conn = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let row = rows::thread(&mut conn, thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let projection = continuation_rows::projection(&mut conn, thread, query.continuation_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !projection.accepts_cursor(
        since > 0 || query.milestones_after.is_some() || query.announcements_after.is_some(),
        query.projection_revision,
    ) {
        return Err(read_failed(
            orch_engine::views::ReadError::ProjectionChanged(
                orch_engine::continuations::projection::scope(&projection),
            ),
        ));
    }
    let view = projection.viewed_continuation_id;
    use orch_db::queries::thread_history as history;
    let counts = history::catchup_counts(&mut conn, thread, view, since).await?;
    let titled = |mut found: Vec<history::Title>| {
        let extra = found.len() > usize::try_from(limit).unwrap_or(usize::MAX);
        if extra {
            found.pop();
        }
        let next = if extra {
            found.last().map(|row| row.seq)
        } else {
            None
        };
        (
            found
                .into_iter()
                .map(|each| CatchupItem {
                    item_id: each.item_id,
                    title: title_of(&each.title),
                })
                .collect(),
            next,
        )
    };
    let (milestones, milestones_next_after) = titled(
        history::titles(
            &mut conn,
            thread,
            view,
            query.milestones_after.unwrap_or(since),
            history::Titles::Milestone,
            i64::from(limit) + 1,
        )
        .await?,
    );
    let (announcements, announcements_next_after) = titled(
        history::titles(
            &mut conn,
            thread,
            view,
            query.announcements_after.unwrap_or(since),
            history::Titles::Announcement,
            i64::from(limit) + 1,
        )
        .await?,
    );
    let reply = Catchup {
        projection: Some(orch_engine::continuations::projection::scope(&projection)),
        since,
        seq: row.seq(),
        items: counts.items,
        turns_completed: counts.turns_completed,
        turns_failed: counts.turns_failed,
        commands: counts.commands,
        files_changed: counts.files_changed,
        approvals_pending: history::approvals_pending(&mut conn, thread, view).await?,
        errors: counts.errors,
        milestones,
        announcements,
        milestones_next_after,
        announcements_next_after,
    };
    conn.commit().await.map_err(orch_db::DbError::from)?;
    crate::middleware::authorization::require_on_path(&ctx, "thread.view").await?;
    Ok(reply)
}

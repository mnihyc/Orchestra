//! The thread feed: the rows of a thread by its sequence, the thread itself, and who is looking.
//!
//! A thread's feed carries its segments, turns, inputs and items as the
//! thread's own request for them (`GET /threads/{id}/sync`) returns them: the
//! rows and caller-specific thread view are read on one snapshot for each credential.
//! The announcement wakes that reader; its actual snapshot head is the delivered cut.
//! The thread itself is added when its own row moved in that range.
//!
//! Who may follow it is `thread.view`; for a side thread, on the thread it was
//! taken from.

use crate::realtime::deltas::Held;
use crate::realtime::subscriptions::{Access, may_view, principal_of};
use crate::realtime::sync::{Cache, Frame, Head, Rows};
use crate::realtime::{Fail, Inner};
use crate::routes::nodes::view;
use orch_api_contract::nodes::Node;
use orch_api_contract::threads::{
    Item, Segment, THREAD_SYNC_MAX_CHANGES, Thread, ThreadContinuationProjection, ThreadInput, Turn,
};
use orch_authz::{Policy, Principal};
use orch_db::Conn;
use orch_db::queries::{feeds, thread_continuations as continuation_rows, threads};
use orch_db::sequences;
use orch_engine::views::ReadError;
use orch_realtime_contract::delta::growing;
use orch_realtime_contract::feeds::{SyncData, ThreadData};
use orch_realtime_contract::sync::FeedHead;
use orch_types::{Id, Seq};
use std::sync::Arc;

/// One immutable subscription selector; its revision belongs to the last admitted cut.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ReadScope {
    pub(crate) continuation: Option<Id>,
    pub(crate) revision: Option<i64>,
}

/// One reader's row cut; it is never reused across credentials or snapshots.
#[derive(Debug)]
struct Snapshot {
    projection: Option<ThreadContinuationProjection>,
    inputs_next_after: Option<Id>,
    inputs_queue_revision: Option<i64>,
    seq: Seq,
    gone: bool,
    complete: bool,
    items: Vec<Item>,
    turns: Vec<Turn>,
    inputs: Vec<ThreadInput>,
    segments: Vec<Segment>,
    /// The stamp of the thread's own row.
    state_stamp: Seq,
    /// The highest stamp among the rows above.
    highest: Option<Seq>,
    held: Vec<Held>,
}

impl Snapshot {
    fn empty() -> Self {
        Self {
            projection: None,
            inputs_next_after: None,
            inputs_queue_revision: None,
            seq: 0,
            gone: false,
            complete: true,
            items: Vec::new(),
            turns: Vec::new(),
            inputs: Vec::new(),
            segments: Vec::new(),
            state_stamp: 0,
            highest: None,
            held: Vec::new(),
        }
    }
}

/// Resolve the sequence and read projection together; a number alone cannot identify history.
pub(crate) async fn head_scoped(
    inner: &Inner,
    thread: Id,
    since: Option<Seq>,
    scope: ReadScope,
) -> Result<Option<(Head, ThreadContinuationProjection, bool)>, Fail> {
    let mut tx = feeds::begin_reading(&inner.db).await?;
    let Some(seq) = sequences::current(&mut tx, thread).await? else {
        return Ok(None);
    };
    let Some(projection) =
        continuation_rows::projection(&mut tx, thread, scope.continuation).await?
    else {
        return Ok(None);
    };
    let current = projection.accepts_cursor(since.is_some(), scope.revision);
    let rows_after = if let Some(since) = since.filter(|since| current && *since <= seq) {
        let rows = inner
            .engine()?
            .rows_since_view_in(&mut tx, thread, since, scope.continuation, scope.revision)
            .await
            .map_err(Fail::other)?;
        if rows.complete {
            i64::try_from(
                rows.items.len() + rows.turns.len() + rows.inputs.len() + rows.segments.len(),
            )
            .unwrap_or(i64::MAX)
        } else {
            THREAD_SYNC_MAX_CHANGES + 1
        }
    } else {
        0
    };
    let projection = orch_engine::continuations::projection::scope(&projection);
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(Some((
        Head {
            feed: FeedHead {
                seq,
                purged_seq: 0,
                incremental_limit: THREAD_SYNC_MAX_CHANGES,
            },
            rows_after,
        },
        projection,
        current,
    )))
}

/// One bounded delta batch may append only to this view's mutable suffix, never its frozen prefix.
pub(crate) async fn live_items(
    inner: &Inner,
    thread: Id,
    scope: ReadScope,
    expected: &ThreadContinuationProjection,
    items: &[Id],
) -> Result<Option<Vec<Id>>, Fail> {
    let mut tx = feeds::begin_reading(&inner.db).await?;
    let Some(projection) =
        continuation_rows::projection(&mut tx, thread, scope.continuation).await?
    else {
        return Ok(None);
    };
    if !projection.accepts_cursor(true, scope.revision)
        || projection.viewed_continuation_id != expected.viewed_continuation_id
        || projection.follows_active != expected.follows_active
    {
        return Ok(None);
    }
    let visible = continuation_rows::reading::mutable_items_among(
        &mut tx,
        thread,
        projection.viewed_continuation_id,
        items,
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(Some(visible))
}

/// Whether `user` may follow the thread.
pub(crate) async fn decide(inner: &Inner, user: Id, thread: Id) -> Result<Access, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some(row) = threads::thread(&mut conn, thread).await? else {
        return Ok(Access::NotFound);
    };
    if !feeds::gone_nodes(&mut conn, &[thread]).await?.is_empty() {
        return Ok(Access::Gone);
    }
    let Some(principal) = principal_of(inner, &mut conn, user).await? else {
        return Ok(Access::Forbidden);
    };
    // A side thread is followed on the strength of the thread it was taken
    // from.
    may_view(&mut conn, &principal, row.side_of.unwrap_or(thread)).await
}

/// Where the thread's feed stands, and how many rows lie above `since`.
pub(crate) async fn head(
    inner: &Inner,
    thread: Id,
    since: Option<Seq>,
) -> Result<Option<Head>, Fail> {
    let mut conn = feeds::begin_reading(&inner.db).await?;
    let Some(seq) = sequences::current(&mut conn, thread).await? else {
        return Ok(None);
    };
    let rows_after = match since {
        Some(since) if since <= seq => {
            threads::count_changed_since(&mut conn, thread, since).await?
        }
        _ => 0,
    };
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(Some(Head {
        feed: FeedHead {
            seq,
            purged_seq: 0,
            incremental_limit: THREAD_SYNC_MAX_CHANGES,
        },
        rows_after,
    }))
}

/// Reads through this snapshot's actual head, as the thread's own request returns them.
async fn load(
    inner: &Inner,
    conn: &mut Conn,
    thread: Id,
    after: Seq,
    scope: ReadScope,
) -> Result<Snapshot, Fail> {
    let engine = inner.engine()?;
    let synced = match engine
        .rows_since_view_in(conn, thread, after, scope.continuation, scope.revision)
        .await
    {
        Ok(synced) => synced,
        Err(ReadError::NotFound) => {
            return Ok(Snapshot {
                gone: true,
                ..Snapshot::empty()
            });
        }
        Err(error) => return Err(Fail::other(error)),
    };
    if !synced.complete {
        return Ok(Snapshot {
            projection: Some(synced.projection),
            seq: synced.seq,
            complete: false,
            ..Snapshot::empty()
        });
    }
    // A notification is a wake-up. Read through this snapshot's actual head; filtering a
    // mutable row back to an older wake-up cannot reconstruct its former value.
    let items = synced.items;
    let turns = synced.turns;
    let inputs = synced.inputs;
    let segments = synced.segments;
    let state_stamp = synced.state_stamp;
    let highest = items
        .iter()
        .map(|item| item.seq_updated)
        .chain(turns.iter().map(|turn| turn.seq_updated))
        .chain(inputs.iter().map(|input| input.seq_updated))
        .chain(segments.iter().map(|segment| segment.seq_updated))
        .max();
    let held = items
        .iter()
        .filter_map(|item| {
            growing(item).map(|(field, len)| Held {
                item: item.id,
                field,
                len,
            })
        })
        .collect();
    Ok(Snapshot {
        projection: Some(synced.projection),
        inputs_next_after: synced.inputs_next_after,
        inputs_queue_revision: Some(synced.inputs_queue_revision),
        seq: synced.seq,
        gone: false,
        complete: true,
        items,
        turns,
        inputs,
        segments,
        state_stamp,
        highest,
        held,
    })
}

/// The thread as `person` sees it, where the application can show it.
async fn thread_view(
    inner: &Inner,
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
) -> Result<Option<Thread>, Fail> {
    let built = view::build(
        conn,
        principal,
        &[thread],
        inner.clock.now(),
        Policy::default(),
    )
    .await
    .map_err(Fail::other)?;
    Ok(built.into_iter().find_map(|each| match each.node {
        Node::Thread(thread) if each.viewable => Some(thread),
        _ => None,
    }))
}

fn some<T>(rows: Vec<T>) -> Option<Vec<T>> {
    if rows.is_empty() { None } else { Some(rows) }
}

/// One subscriber's rows through a coherent head at least as new as the wake-up `upto`.
#[cfg(test)]
pub(crate) async fn read(
    inner: &Arc<Inner>,
    person: Id,
    thread: Id,
    after: Seq,
    upto: Seq,
    cache: &mut Cache,
) -> Result<Rows, Fail> {
    read_scoped(
        inner,
        person,
        thread,
        after,
        upto,
        cache,
        ReadScope::default(),
    )
    .await
}

pub(crate) async fn read_scoped(
    inner: &Arc<Inner>,
    person: Id,
    thread: Id,
    after: Seq,
    upto: Seq,
    _cache: &mut Cache,
    scope: ReadScope,
) -> Result<Rows, Fail> {
    // Personalized rows and authority share one cut. Do not reuse another credential's
    // snapshot, even when its user and numeric cursor happen to match.
    let mut tx = feeds::begin_reading(&inner.db).await?;
    let snapshot = load(inner, &mut tx, thread, after, scope).await?;
    #[cfg(test)]
    super::history_read_tests::pause(super::history_read_tests::Cut::SocketRows).await;
    if snapshot.gone {
        return Ok(Rows::Gone);
    }
    if snapshot.projection.as_ref().is_some_and(|projection| {
        scope
            .revision
            .is_some_and(|revision| revision != projection.projection_revision)
            || scope.revision.is_none()
                && projection.follows_active
                && projection.execution_revision > 1
    }) {
        return Ok(Rows::ProjectionChanged);
    }
    if !snapshot.complete || snapshot.seq < upto {
        return Ok(Rows::TooMany);
    }
    let Some(principal) = principal_of(inner, &mut tx, person).await? else {
        return Ok(Rows::Nothing);
    };
    let projection = snapshot
        .projection
        .as_ref()
        .ok_or_else(|| Fail::other("The thread read has no projection identity."))?;
    let changed = orch_engine::continuations::projection::changed_ids(
        &mut tx,
        thread,
        projection.viewed_continuation_id,
        Some(after),
        THREAD_SYNC_MAX_CHANGES + 1,
    )
    .await
    .map_err(Fail::other)?;
    if i64::try_from(changed.len()).unwrap_or(i64::MAX) > THREAD_SYNC_MAX_CHANGES {
        return Ok(Rows::TooMany);
    }
    let mut ids = orch_engine::continuations::projection::item_inputs(&snapshot.items);
    ids.extend(changed);
    let metadata = orch_engine::continuations::projection::metadata(
        inner.engine()?,
        &mut tx,
        &principal,
        thread,
        projection.viewed_continuation_id,
        ids,
    )
    .await
    .map_err(Fail::other)?;
    let mut highest = snapshot.highest;
    if !metadata.summaries.is_empty() || metadata.viewed_prompt.is_some() {
        highest = highest.max(Some(snapshot.seq));
    }
    let mut own = None;
    if snapshot.state_stamp > after
        && let Some(mut found) = thread_view(inner, &mut tx, &principal, thread).await?
    {
        found.my_last_read_seq = continuation_rows::reading::last_read(
            &mut tx,
            person,
            thread,
            projection.viewed_continuation_id,
        )
        .await?;
        highest = highest.max(Some(snapshot.state_stamp));
        own = Some(found);
    }
    let Some(highest) = highest else {
        return Ok(Rows::Nothing);
    };
    // Only this credential's items receive its approval authority.
    let mut items = snapshot.items;
    if items.iter().any(|item| {
        matches!(
            &item.payload,
            orch_api_contract::threads::ItemPayload::Approval(_)
        )
    }) {
        crate::routes::threads::approval_items::project(
            &mut tx,
            &principal,
            thread,
            &mut items,
            inner.clock.now(),
            inner.engine()?,
        )
        .await
        .map_err(Fail::other)?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(Rows::Frame(Box::new(Frame {
        data: SyncData::Thread(ThreadData {
            projection: snapshot.projection,
            prompt_summaries: metadata.summaries,
            viewed_prompt: metadata.viewed_prompt,
            inputs_next_after: snapshot.inputs_next_after,
            inputs_queue_revision: snapshot.inputs_queue_revision,
            complete: Some(true),
            thread: own,
            items: some(items),
            turns: some(snapshot.turns),
            inputs: some(snapshot.inputs),
            segments: some(snapshot.segments),
        }),
        highest: snapshot.seq.max(highest),
        held: snapshot.held,
    })))
}

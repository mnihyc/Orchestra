//! Newly committed harness frames forwarded with the same masking as the raw-frame list.
//!
//! Capture the segment's starting number in the ingestion transaction, then list only the new
//! frames after it commits. Reading through the engine applies current masking a second time;
//! the unmasked harness payload is never put on the transient backplane.

use super::ephemeral::{Message, broadcast};
use super::subscriptions::{SubState, Subscription};
use super::{Fail, Inner};
use orch_api_contract::threads::{RawEvent, ThreadContinuationProjection};
use orch_db::{Conn, Db};
use orch_link::streams::Delivery;
use orch_link_contract::streams::Stream;
use orch_realtime_contract::feeds::RawData;
use orch_realtime_contract::feeds::{Feed, ResourceKind};
use orch_realtime_contract::frames::{
    RawSubscription, ServerFrame, SocketErrorCode, SubscribedFrame,
};
use orch_realtime_contract::sync::SubscribeMode;
use orch_types::Id;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Only the newest row is needed to mark the boundary, including after retention removed older rows.
const BOUNDARY_ROWS: i64 = 1;

/// The boundary before a delivery's raw frames were applied.
pub struct Boundary {
    thread: Id,
    segment: Id,
    n: i64,
}

/// Takes the boundary only when this delivery contains diagnostic frames.
pub async fn before(
    conn: &mut Conn,
    delivery: &Delivery,
) -> Result<Option<Boundary>, orch_db::DbError> {
    let Stream::Session {
        thread_id,
        segment_id,
    } = &delivery.stream
    else {
        return Ok(None);
    };
    if !delivery
        .events
        .iter()
        .any(|event| event.kind == "raw.frame")
    {
        return Ok(None);
    }
    let n = orch_db::queries::raw_events::newest(conn, *segment_id, None, BOUNDARY_ROWS)
        .await?
        .first()
        .map_or(0, |row| row.n);
    Ok(Some(Boundary {
        thread: *thread_id,
        segment: *segment_id,
        n,
    }))
}

/// Announces every new masked frame and every committed provider sign-in step in order.
pub async fn after(db: &Db, boundary: Option<Boundary>, delivery: &Delivery) -> Result<(), Fail> {
    if let Some(boundary) = boundary {
        let mut conn = db.acquire().await?;
        let Some(thread) = orch_db::queries::threads::thread(&mut conn, boundary.thread).await?
        else {
            return Ok(());
        };
        let mut frames = Vec::new();
        let mut before = None;
        loop {
            let page = orch_engine::raw_events::page(
                &mut conn,
                &thread,
                boundary.segment,
                before,
                orch_engine::raw_events::PAGE_MAX,
            )
            .await
            .map_err(Fail::other)?;
            let done =
                !page.has_older || page.events.last().is_none_or(|frame| frame.n <= boundary.n);
            before = page.events.last().map(|frame| frame.n);
            frames.extend(page.events.into_iter().filter(|frame| frame.n > boundary.n));
            if done {
                break;
            }
        }
        drop(conn);
        publish(
            db,
            boundary.thread,
            boundary.segment,
            frames.into_iter().rev(),
        )
        .await?;
    }
    if let Stream::Provider { provider_id: id } = &delivery.stream {
        for event in &delivery.events {
            if event.kind == "provider.login" {
                // Decode first so implementation bookkeeping never leaks into the public state.
                let state: orch_api_contract::resources::LoginState =
                    serde_json::from_value(event.body.clone()).map_err(Fail::other)?;
                broadcast(
                    db,
                    &Message::Event {
                        feed: Feed::Resource {
                            kind: ResourceKind::Provider,
                            id: *id,
                        },
                        data: serde_json::json!({"what": "login", "state": state}),
                    },
                )
                .await?;
            }
        }
    }
    Ok(())
}

/// A bounded already-masked source batch; no unmasked native payload enters the transient channel.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Batch {
    pub(crate) thread: Id,
    pub(crate) segment: Id,
    pub(crate) events: Vec<RawEvent>,
}

async fn publish(
    db: &Db,
    thread: Id,
    segment: Id,
    events: impl IntoIterator<Item = RawEvent>,
) -> Result<(), Fail> {
    let mut batch = Batch {
        thread,
        segment,
        events: Vec::new(),
    };
    let overhead = serde_json::to_vec(&Message::Raw {
        batch: Batch {
            thread,
            segment,
            events: Vec::new(),
        },
    })
    .map_err(Fail::other)?
    .len();
    let mut bytes = overhead;
    for event in events {
        let size = serde_json::to_vec(&event).map_err(Fail::other)?.len();
        if overhead.saturating_add(size) > super::ephemeral::MESSAGE_BYTES {
            return Err(Fail::other(
                "one masked raw event exceeds the transient frame limit",
            ));
        }
        let comma = usize::from(!batch.events.is_empty());
        if bytes.saturating_add(comma).saturating_add(size) > super::ephemeral::MESSAGE_BYTES
            || batch.events.len()
                >= usize::try_from(orch_engine::raw_events::PAGE_MAX).unwrap_or(usize::MAX)
        {
            broadcast(db, &Message::Raw { batch }).await?;
            batch = Batch {
                thread,
                segment,
                events: Vec::new(),
            };
            bytes = overhead;
        }
        bytes = bytes
            .saturating_add(usize::from(!batch.events.is_empty()))
            .saturating_add(size);
        batch.events.push(event);
    }
    if !batch.events.is_empty() {
        broadcast(db, &Message::Raw { batch }).await?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
enum Snapshot {
    Missing,
    Present {
        projection: ThreadContinuationProjection,
        after: i64,
        visible: bool,
        complete: bool,
    },
}

type SourceKey = (Id, Id, Option<Id>);
type Sources = HashMap<SourceKey, Snapshot>;

/// Each distinct segment/history is resolved once in a delivery batch, never once per raw row.
async fn snapshot(inner: &Inner, key: SourceKey, cache: &mut Sources) -> Result<Snapshot, Fail> {
    if let Some(found) = cache.get(&key) {
        return Ok(found.clone());
    }
    let (thread, segment, selection) = key;
    let mut tx = orch_db::queries::feeds::begin_reading(&inner.db).await?;
    let found = if let Some(projection) =
        orch_db::queries::thread_continuations::projection(&mut tx, thread, selection).await?
    {
        let belongs = orch_db::queries::threads::segment(&mut tx, segment)
            .await?
            .is_some_and(|row| row.thread_id == thread);
        let visible = belongs
            && !orch_db::queries::thread_continuations::reading::segments_among(
                &mut tx,
                thread,
                projection.viewed_continuation_id,
                &[segment],
            )
            .await?
            .is_empty();
        if belongs {
            let complete = visible
                && orch_db::queries::thread_history::complete_segment(
                    &mut tx,
                    thread,
                    projection.viewed_continuation_id,
                    segment,
                )
                .await?;
            let after = orch_db::queries::raw_events::last_number(&mut tx, segment).await?;
            Snapshot::Present {
                projection: orch_engine::continuations::projection::scope(&projection),
                after,
                visible,
                complete,
            }
        } else {
            Snapshot::Missing
        }
    } else {
        Snapshot::Missing
    };
    tx.commit().await.map_err(orch_db::DbError::from)?;
    cache.insert(key, found.clone());
    Ok(found)
}

fn same_projection(
    left: &ThreadContinuationProjection,
    right: &ThreadContinuationProjection,
) -> bool {
    left.viewed_continuation_id == right.viewed_continuation_id
        && left.projection_revision == right.projection_revision
        && left.follows_active == right.follows_active
        && (!left.follows_active
            || left.active_continuation_id == right.active_continuation_id
                && left.execution_revision == right.execution_revision)
}

/// Admit an immutable raw source, with a live-only number floor. No past raw rows are sent here.
pub(crate) async fn begin(
    inner: &Arc<Inner>,
    sub: &Arc<Subscription>,
    st: &mut SubState,
) -> Result<bool, Fail> {
    let Feed::ThreadRaw(thread) = sub.feed() else {
        return Ok(false);
    };
    let Some(segment) = sub.raw_segment() else {
        return Ok(false);
    };
    let scope = sub.thread_scope(st);
    let found = snapshot(
        inner,
        (*thread, segment, scope.continuation),
        &mut Sources::new(),
    )
    .await?;
    // The source read may await the database; revalidate even before returning its eligibility refusal.
    let epoch = super::sync::read_epoch(inner).await?;
    if !inner.recheck(sub, st, epoch).await? {
        return Ok(false);
    }
    let Snapshot::Present {
        projection,
        after,
        visible,
        complete,
    } = found
    else {
        sub.connection().error(
            Some(sub.tag().id),
            SocketErrorCode::NotFound,
            "There is no raw segment in that history.",
        );
        return Ok(false);
    };
    let current = scope.revision == Some(projection.projection_revision);
    if current && !visible {
        sub.connection().error(
            Some(sub.tag().id),
            SocketErrorCode::NotFound,
            "There is no raw segment in that history.",
        );
        return Ok(false);
    }
    if current && !complete {
        sub.connection().error(Some(sub.tag().id),SocketErrorCode::ReadUnavailable,
            "These raw protocol frames describe the whole native session. Open its original history to read them.");
        return Ok(false);
    }
    st.thread_projection = Some(projection.clone());
    st.raw_after = after;
    let frame = ServerFrame::Subscribed(SubscribedFrame {
        raw: Some(RawSubscription {
            segment_id: segment,
            after,
        }),
        projection: Some(projection),
        id: sub.tag().id,
        feed: sub.feed().clone(),
        seq: None,
        mode: if current {
            SubscribeMode::Ephemeral
        } else {
            SubscribeMode::Reload
        },
    });
    sub.connection()
        .outbox
        .push_frame(sub.tag(), frame.encode().map_err(Fail::other)?);
    Ok(current)
}

async fn current(
    inner: &Arc<Inner>,
    sub: &Arc<Subscription>,
    st: &mut SubState,
    cache: &mut Sources,
) -> Result<Option<ThreadContinuationProjection>, Fail> {
    if !st.open || !sub.is_live() {
        return Ok(None);
    }
    let Some(_identity) = super::watches::delivery_guard(inner, sub.connection()).await? else {
        return Ok(None);
    };
    let epoch = super::sync::read_epoch(inner).await?;
    if !inner.recheck(sub, st, epoch).await? || !sub.is_live() {
        return Ok(None);
    }
    let Feed::ThreadRaw(thread) = sub.feed() else {
        return Ok(None);
    };
    let Some(segment) = sub.raw_segment() else {
        return Ok(None);
    };
    let scope = sub.thread_scope(st);
    let found = snapshot(inner, (*thread, segment, scope.continuation), cache).await?;
    // Every metadata transaction has ended before the second pooled authority check.
    let epoch = super::sync::read_epoch(inner).await?;
    if !inner.recheck(sub, st, epoch).await? || !sub.is_live() {
        return Ok(None);
    }
    match found {
        Snapshot::Present {
            projection,
            complete: true,
            ..
        } if st
            .thread_projection
            .as_ref()
            .is_some_and(|old| same_projection(old, &projection)) =>
        {
            st.thread_projection = Some(projection.clone());
            Ok(Some(projection))
        }
        Snapshot::Missing => {
            inner
                .close_sub(
                    sub,
                    st,
                    orch_realtime_contract::frames::FeedClosedReason::Deleted,
                )
                .await;
            Ok(None)
        }
        Snapshot::Present { .. } => {
            inner.invalidate_thread_projection(sub, st);
            Ok(None)
        }
    }
}

/// Apply the GET's current masking once per bounded source batch, including a batch queued across a policy change.
async fn remasked(inner: &Inner, batch: &Batch) -> Result<Vec<RawEvent>, Fail> {
    if batch.events.is_empty() {
        return Ok(Vec::new());
    }
    let wanted: HashSet<i64> = batch.events.iter().map(|event| event.n).collect();
    let last = batch
        .events
        .iter()
        .map(|event| event.n)
        .max()
        .ok_or_else(|| Fail::other("raw source batch is empty"))?;
    let limit = u32::try_from(batch.events.len()).map_err(Fail::other)?;
    if limit > orch_engine::raw_events::PAGE_MAX {
        return Err(Fail::other("raw source batch exceeds its page bound"));
    }
    let mut tx = orch_db::queries::feeds::begin_reading(&inner.db).await?;
    let page = if let Some(thread) =
        orch_db::queries::threads::thread(&mut tx, batch.thread).await?
    {
        orch_engine::raw_events::page(&mut tx, &thread, batch.segment, last.checked_add(1), limit)
            .await
            .map_err(Fail::other)?
            .events
    } else {
        Vec::new()
    };
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(page
        .into_iter()
        .rev()
        .filter(|event| wanted.contains(&event.n))
        .collect())
}

/// Recipient-specific projection/authority is checked once per source batch, then encoded without awaits per row.
pub(crate) async fn deliver(inner: &Arc<Inner>, batch: Batch) -> Result<(), Fail> {
    let subscribers: Vec<_> = inner
        .ephemeral
        .raw_subscribers(Some(batch.thread))
        .into_iter()
        .filter(|sub| sub.raw_segment() == Some(batch.segment))
        .collect();
    if subscribers.is_empty() {
        return Ok(());
    }
    let events = remasked(inner, &batch).await?;
    let mut sources = Sources::new();
    for sub in subscribers {
        let mut st = sub.state.lock().await;
        let Some(projection) = current(inner, &sub, &mut st, &mut sources).await? else {
            continue;
        };
        for event in &events {
            if event.n <= st.raw_after {
                continue;
            }
            let value = serde_json::to_value(RawData {
                segment_id: batch.segment,
                projection: projection.clone(),
                event: event.clone(),
            })
            .map_err(Fail::other)?;
            super::ephemeral::send(&sub, &value)?;
            st.raw_after = event.n;
        }
    }
    Ok(())
}

/// Existing commit notifications reset follow-active sources, including A -> B -> A with no intervening raw frame.
pub(crate) async fn refresh(inner: &Arc<Inner>, thread: Option<Id>) -> Result<(), Fail> {
    let mut sources = Sources::new();
    for sub in inner.ephemeral.raw_subscribers(thread) {
        let mut st = sub.state.lock().await;
        let _ = current(inner, &sub, &mut st, &mut sources).await?;
    }
    Ok(())
}

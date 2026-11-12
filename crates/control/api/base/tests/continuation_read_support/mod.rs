//! Retained projection fixtures over genuinely admitted history, without claiming native activation proof.
#![allow(
    dead_code,
    reason = "each test program that includes these fixtures uses only some of them"
)]

use orch_db::{
    Db,
    queries::{thread_continuations as rows, threads},
    sequences,
};
use orch_types::{Id, Seq, Time, new_id};

pub type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[allow(
    dead_code,
    reason = "shared fixture identities are read by different integration test binaries"
)]
pub struct Retained {
    pub root: Id,
    pub child: Id,
    pub first_input: Id,
    pub before_input: Id,
    pub prefix_item: Id,
}

/// Freeze before the second actual native turn. This authors only read-fixture metadata;
/// domain/adapter tests separately prove preparation and selection through the real job.
pub async fn retained(db: &Db, thread: Id, actor: Id, now: Time) -> Proof<Retained> {
    let mut tx = db.begin().await?;
    let head = threads::lock(&mut tx, thread)
        .await?
        .ok_or("missing fixture thread")?;
    if head.current_turn_id.is_some() || threads::turn_end_work_pending(&mut tx, thread).await? {
        return Err(
            "the native fixture must finish its real turns and accounting before freezing history"
                .into(),
        );
    }
    let (before_input, ordinal, position): (Id, i32, Seq) = sqlx::query_as(
        "SELECT t.input_id,t.ordinal,i.seq_created
        FROM turns t JOIN items i ON i.turn_id=t.id AND i.stream='main' AND i.kind='user_message'
        AND i.payload->>'input_id'=t.input_id::text WHERE t.thread_id=$1 AND t.ordinal=2",
    )
    .bind(thread)
    .fetch_one(&mut *tx)
    .await?;
    let (first_input, prefix_item): (Id, Id) = sqlx::query_as(
        "SELECT t.input_id,i.id FROM turns t
        JOIN items i ON i.turn_id=t.id AND i.stream='main' AND i.kind='agent_message'
        WHERE t.thread_id=$1 AND t.ordinal=1 ORDER BY i.seq_created LIMIT 1",
    )
    .bind(thread)
    .fetch_one(&mut *tx)
    .await?;
    let seq = sequences::next(&mut tx, thread)
        .await?
        .ok_or("missing fixture sequence")?;
    let child = new_id();
    rows::insert(
        &mut tx,
        &rows::NewContinuation {
            id: child,
            thread,
            parent: head.active_continuation_id,
            before_input,
            actor,
            at: now.get(),
            seq,
            queue: rows::QueueRow {
                held: true,
                held_by: Some(actor),
                held_at: Some(now.get()),
                revision: 1,
            },
        },
    )
    .await?;
    rows::freeze_prefix(
        &mut tx,
        thread,
        head.active_continuation_id,
        child,
        ordinal,
        position,
        seq,
    )
    .await?;
    sqlx::query("UPDATE thread_continuations SET status='ready' WHERE id=$1")
        .bind(child)
        .execute(&mut *tx)
        .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(Retained {
        root: head.active_continuation_id,
        child,
        first_input,
        before_input,
        prefix_item,
    })
}

/// Only the test's projection namespace changes; no fake native preparation or delivered turn is invented.
pub async fn select(db: &Db, thread: Id, view: Id, now: Time) -> Proof<Seq> {
    let mut tx = db.begin().await?;
    let current = threads::lock(&mut tx, thread)
        .await?
        .ok_or("missing fixture thread")?;
    if current.current_turn_id.is_some() {
        return Err("a projection fixture cannot replace running native work".into());
    }
    let seq = sequences::next(&mut tx, thread)
        .await?
        .ok_or("missing fixture sequence")?;
    let changed=sqlx::query("UPDATE threads t SET active_continuation_id=c.id,execution_revision=t.execution_revision+1,
        current_segment_id=c.current_segment_id,current_turn_id=NULL,seq_state=$3,
        queue_held=c.queue_held,queue_held_by=c.queue_held_by,queue_held_at=c.queue_held_at,queue_revision=c.queue_revision
        FROM thread_continuations c WHERE t.node_id=$1 AND c.thread_id=t.node_id AND c.id=$2 AND c.status='ready'")
        .bind(thread).bind(view).bind(seq).execute(&mut *tx).await?.rows_affected();
    if changed != 1 {
        return Err("the retained fixture projection is not ready in this thread".into());
    }
    rows::reset_selected_observations(&mut tx, thread, now.get()).await?;
    sequences::notify(&mut tx, thread, seq).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(seq)
}

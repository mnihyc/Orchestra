//! Deterministic snapshot boundaries with the native HTTP, machine and thread projections.

use super::{
    Inner,
    sync::{Cache, Frame, Rows},
    threads,
};
use crate::rig::{self, PATIENCE, Scene};
use orch_api_contract::approvals::{
    ApprovalDecide, ApprovalDecision, ApprovalState, HarnessOptionDecision,
};
use orch_api_contract::threads::{
    Delivery, ItemPayload, ThreadOpeningGet, ThreadOpeningGetQuery, ThreadState, ThreadSync,
    ThreadSyncGet, ThreadSyncGetQuery, TurnState,
};
use orch_link_contract::commands::TurnStart;
use orch_link_contract::events::TurnOutcome;
use orch_realtime_contract::feeds::{SyncData, ThreadData};
use orch_types::{Id, Seq};
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::oneshot;

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cut {
    HttpHead,
    SocketRows,
}

struct Boundary {
    cut: Cut,
    reached: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
}

tokio::task_local! {
    // A barrier belongs to this single test reader, never a process-wide request switch.
    static BOUNDARY: Mutex<Option<Boundary>>;
}

pub(crate) async fn pause(cut: Cut) {
    let boundary = BOUNDARY
        .try_with(|slot| {
            let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
            if slot.as_ref().is_some_and(|boundary| boundary.cut == cut) {
                slot.take()
            } else {
                None
            }
        })
        .ok()
        .flatten();
    if let Some(boundary) = boundary {
        let _ = boundary.reached.send(());
        let _ = boundary.resume.await;
    }
}

async fn interleave<R, W>(
    cut: Cut,
    read: impl Future<Output = R>,
    write: impl Future<Output = Proof<W>>,
) -> Proof<(R, W)> {
    let (reached, wait) = oneshot::channel();
    let (resume, blocked) = oneshot::channel();
    let reader = BOUNDARY.scope(
        Mutex::new(Some(Boundary {
            cut,
            reached,
            resume: blocked,
        })),
        read,
    );
    let writer = async {
        wait.await?;
        let written = write.await?;
        resume
            .send(())
            .map_err(|()| "the paused reader ended before its writer committed")?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(written)
    };
    let (read, written) =
        tokio::time::timeout(PATIENCE, async { tokio::join!(reader, writer) }).await?;
    Ok((read, written?))
}

async fn running() -> (Scene, Id, TurnStart) {
    let scene = Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene
        .say(
            &scene.ada,
            thread,
            "Show the snapshot boundary",
            Delivery::Queue,
            "read-cut-input",
        )
        .await
        .ok();
    let start = scene.rig.turn_start(thread, 0).await;
    scene
        .rig
        .answer(
            thread,
            &start,
            "read-cut-answer",
            "Before the concurrent commit",
        )
        .await;
    scene.rig.wait_turn(start.turn_id, "running").await;
    scene.rig.wait_state(thread, "running").await;
    // Publication and Running are earlier than the last message's committed delivery and
    // the turn-start command's receipt. None of that setup belongs to the tested interval.
    rig::eventually("the initial native answer and input receipt are committed", || async {
        sqlx::query_scalar::<_, bool>("SELECT EXISTS (
            SELECT 1 FROM items i JOIN model_inputs m ON m.node_id=i.thread_id AND m.turn_id=i.turn_id
            WHERE i.thread_id=$1 AND i.turn_id=$2 AND i.native_ref='read-cut-answer'
              AND i.kind='agent_message' AND i.status='completed'
              AND i.payload->>'text'='Before the concurrent commit'
              AND m.purpose='input' AND m.state='sent'
              AND NOT EXISTS (SELECT 1 FROM model_inputs pending
                  WHERE pending.node_id=$1 AND pending.state='prepared'))")
            .bind(thread).bind(start.turn_id).fetch_one(scene.rig.db.pool()).await
            .unwrap_or_else(|error| panic!("could not read the native setup's committed state: {error}"))
    }).await;
    (scene, thread, start)
}

async fn sync(scene: &Scene, thread: Id, since: Option<Seq>) -> ThreadSync {
    scene
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadSyncGetQuery {
            since,
            ..Default::default()
        })
        .send()
        .await
        .ok()
}

async fn complete(scene: &Scene, thread: Id, start: &TurnStart) -> Proof<Seq> {
    scene
        .rig
        .say(
            thread,
            rig::turn_ended(start.turn_id, TurnOutcome::Completed),
        )
        .await;
    scene.rig.wait_turn(start.turn_id, "completed").await;
    scene.rig.wait_state(thread, "idle").await;
    let mut conn = scene.rig.world.conn().await;
    orch_db::sequences::current(&mut conn, thread)
        .await?
        .ok_or_else(|| "thread disappeared".into())
}

fn bounded(sync: &ThreadSync) {
    assert_eq!(sync.seq, sync.thread.seq);
    assert!(sync.items.iter().all(|row| row.seq_updated <= sync.seq));
    assert!(sync.turns.iter().all(|row| row.seq_updated <= sync.seq));
    assert!(sync.inputs.iter().all(|row| row.seq_updated <= sync.seq));
    assert!(sync.segments.iter().all(|row| row.seq_updated <= sync.seq));
}

fn socket_inner(scene: &Scene) -> Arc<Inner> {
    let app = &scene.rig.world.app.app;
    app.realtime.attach_app(app);
    // Projection tests need the real engine, but no fan-out timers or server loop.
    assert!(
        app.realtime
            .inner
            .engine
            .set(scene.rig.engine.clone())
            .is_ok()
    );
    Arc::clone(&app.realtime.inner)
}

fn frame(rows: Rows) -> Proof<Box<Frame>> {
    match rows {
        Rows::Frame(frame) => Ok(frame),
        _ => Err("the native thread change did not produce a frame".into()),
    }
}

fn data(frame: &Frame) -> Proof<&ThreadData> {
    match &frame.data {
        SyncData::Thread(data) => {
            assert!(
                data.items
                    .iter()
                    .flatten()
                    .all(|row| row.seq_updated <= frame.highest)
            );
            assert!(
                data.turns
                    .iter()
                    .flatten()
                    .all(|row| row.seq_updated <= frame.highest)
            );
            assert!(
                data.inputs
                    .iter()
                    .flatten()
                    .all(|row| row.seq_updated <= frame.highest)
            );
            assert!(
                data.segments
                    .iter()
                    .flatten()
                    .all(|row| row.seq_updated <= frame.highest)
            );
            if let Some(thread) = &data.thread {
                assert_eq!(thread.seq, frame.highest);
            }
            Ok(data)
        }
        _ => Err("a different feed was returned for a thread".into()),
    }
}

#[tokio::test]
async fn http_sync_head_count_and_rows_stay_on_one_cut_across_a_terminal_commit() -> Proof {
    for since in [None, Some(0)] {
        let (scene, thread, start) = running().await;
        let (reply, terminal) = interleave(
            Cut::HttpHead,
            sync(&scene, thread, since),
            complete(&scene, thread, &start),
        )
        .await?;
        bounded(&reply);
        assert!(reply.complete);
        assert!(reply.seq < terminal);
        assert_eq!(reply.thread.state, ThreadState::Running);
        assert_eq!(reply.thread.current_turn_id, Some(start.turn_id));
        assert_eq!(
            reply
                .turns
                .iter()
                .find(|turn| turn.id == start.turn_id)
                .ok_or("running turn absent")?
                .state,
            TurnState::Running
        );
        let after = sync(&scene, thread, Some(reply.seq)).await;
        bounded(&after);
        assert!(after.seq >= terminal);
        assert_eq!(after.thread.state, ThreadState::Idle);
        assert_eq!(after.thread.current_turn_id, None);
        assert_eq!(
            after
                .turns
                .iter()
                .find(|turn| turn.id == start.turn_id)
                .ok_or("terminal turn absent")?
                .state,
            TurnState::Completed
        );
    }
    Ok(())
}

#[tokio::test]
async fn socket_rows_and_personalized_thread_do_not_mix_two_commits() -> Proof {
    let (scene, thread, start) = running().await;
    let inner = socket_inner(&scene);
    let wake = sync(&scene, thread, None).await.seq;
    let mut cache = Cache::default();
    let (rows, terminal) = interleave(
        Cut::SocketRows,
        threads::read(&inner, scene.ada.id(), thread, 0, wake, &mut cache),
        complete(&scene, thread, &start),
    )
    .await?;
    let old = frame(rows?)?;
    let old_data = data(&old)?;
    assert!(old.highest < terminal);
    let old_thread = old_data
        .thread
        .as_ref()
        .ok_or("personalized thread absent")?;
    assert_eq!(old_thread.state, ThreadState::Running);
    assert_eq!(old_thread.current_turn_id, Some(start.turn_id));
    assert_eq!(
        old_data
            .turns
            .iter()
            .flatten()
            .find(|turn| turn.id == start.turn_id)
            .ok_or("running turn absent")?
            .state,
        TurnState::Running
    );
    let next = frame(
        threads::read(
            &inner,
            scene.ada.id(),
            thread,
            old.highest,
            terminal,
            &mut cache,
        )
        .await?,
    )?;
    let next_data = data(&next)?;
    let next_thread = next_data
        .thread
        .as_ref()
        .ok_or("terminal personalized thread absent")?;
    assert_eq!(next_thread.state, ThreadState::Idle);
    assert_eq!(next_thread.current_turn_id, None);
    assert_eq!(
        next_data
            .turns
            .iter()
            .flatten()
            .find(|turn| turn.id == start.turn_id)
            .ok_or("terminal turn absent")?
            .state,
        TurnState::Completed
    );
    Ok(())
}

#[tokio::test]
async fn a_late_wake_carries_every_changed_row_through_the_actual_snapshot_head() -> Proof {
    let (scene, thread, start) = running().await;
    let inner = socket_inner(&scene);
    let wake = sync(&scene, thread, None).await.seq;
    complete(&scene, thread, &start).await?;
    // One controlled transaction changes all four row families after the old notification.
    // These are persisted synthetic reports; no provider or machine call is held inside it.
    let mut tx = scene.rig.db.begin().await?;
    let segment: Id = sqlx::query_scalar("SELECT current_segment_id FROM threads WHERE node_id=$1")
        .bind(thread)
        .fetch_one(&mut *tx)
        .await?;
    let item: Id = sqlx::query_scalar("SELECT id FROM items WHERE thread_id=$1 AND kind='agent_message' ORDER BY seq_created DESC LIMIT 1")
        .bind(thread).fetch_one(&mut *tx).await?;
    let input: Id = sqlx::query_scalar("SELECT input_id FROM turns WHERE id=$1")
        .bind(start.turn_id)
        .fetch_one(&mut *tx)
        .await?;
    let item_seq = orch_db::sequences::next(&mut tx, thread)
        .await?
        .ok_or("thread disappeared")?;
    sqlx::query("UPDATE items SET payload=payload || '{\"text\":\"After the old wake\",\"text_len\":18}'::jsonb,seq_updated=$2 WHERE id=$1")
        .bind(item).bind(item_seq).execute(&mut *tx).await?;
    let turn_seq = orch_db::sequences::next(&mut tx, thread)
        .await?
        .ok_or("thread disappeared")?;
    sqlx::query("UPDATE turns SET seq_updated=$2 WHERE id=$1")
        .bind(start.turn_id)
        .bind(turn_seq)
        .execute(&mut *tx)
        .await?;
    let input_seq = orch_db::sequences::next(&mut tx, thread)
        .await?
        .ok_or("thread disappeared")?;
    sqlx::query("UPDATE thread_inputs SET seq_updated=$2 WHERE id=$1")
        .bind(input)
        .bind(input_seq)
        .execute(&mut *tx)
        .await?;
    let segment_seq = orch_db::sequences::next(&mut tx, thread)
        .await?
        .ok_or("thread disappeared")?;
    sqlx::query("UPDATE segments SET seq_updated=$2 WHERE id=$1")
        .bind(segment)
        .bind(segment_seq)
        .execute(&mut *tx)
        .await?;
    let cut = orch_db::sequences::next(&mut tx, thread)
        .await?
        .ok_or("thread disappeared")?;
    orch_db::queries::threads::set_seq_state(&mut tx, thread, cut).await?;
    tx.commit().await?;
    let mut cache = Cache::default();
    let read =
        frame(threads::read(&inner, scene.ada.id(), thread, wake - 1, wake, &mut cache).await?)?;
    assert!(read.highest >= cut);
    let read_data = data(&read)?;
    assert!(
        read_data
            .items
            .iter()
            .flatten()
            .any(|row| row.id == item && row.seq_updated == item_seq)
    );
    assert!(
        read_data
            .turns
            .iter()
            .flatten()
            .any(|row| row.id == start.turn_id
                && row.seq_updated >= turn_seq
                && row.state == TurnState::Completed)
    );
    assert!(
        read_data
            .inputs
            .iter()
            .flatten()
            .any(|row| row.id == input && row.seq_updated == input_seq)
    );
    assert!(
        read_data
            .segments
            .iter()
            .flatten()
            .any(|row| row.id == segment && row.seq_updated == segment_seq)
    );
    assert_eq!(
        read_data
            .thread
            .as_ref()
            .ok_or("personalized thread absent")?
            .state,
        ThreadState::Idle
    );
    Ok(())
}

#[tokio::test]
async fn http_incremental_budget_and_rows_use_the_same_snapshot_as_its_head() -> Proof {
    let (scene, thread, _) = running().await;
    let before = sync(&scene, thread, None).await;
    let write = async {
        let mut tx = scene.rig.db.begin().await?;
        let segment = before
            .thread
            .current_segment_id
            .ok_or("current segment absent")?;
        // One more than the existing incremental budget, with a real unique creation stamp per row.
        let ids: Vec<_> = (0..=orch_api_contract::threads::THREAD_SYNC_MAX_CHANGES)
            .map(|_| orch_types::new_id())
            .collect();
        let count = i64::try_from(ids.len())?;
        let first: Seq = sqlx::query_scalar(
            "UPDATE threads SET next_seq=next_seq+$2 WHERE node_id=$1 RETURNING next_seq-$2",
        )
        .bind(thread)
        .bind(count)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO items(id,thread_id,segment_id,stream,kind,status,payload,seq_created,seq_updated,created_at,updated_at)
            SELECT id,$1,$2,'main','agent_message','completed','{\"text\":\"Count boundary\",\"text_len\":14}'::jsonb,
                $3+ordinal-1,$3+ordinal-1,$5,$5 FROM unnest($4::uuid[]) WITH ORDINALITY AS added(id,ordinal)")
            .bind(thread).bind(segment).bind(first).bind(&ids).bind(scene.rig.world.now())
            .execute(&mut *tx).await?;
        let last = first + count - 1;
        orch_db::queries::threads::set_seq_state(&mut tx, thread, last).await?;
        tx.commit().await?;
        Ok(last)
    };
    let (old, last) =
        interleave(Cut::HttpHead, sync(&scene, thread, Some(before.seq)), write).await?;
    bounded(&old);
    assert_eq!(old.since, Some(before.seq));
    assert!(old.seq >= before.seq);
    assert!(
        old.complete,
        "a later large commit cannot change the earlier count gate"
    );
    assert!(old.items.is_empty());
    // An incremental page also carries the exact turn/segment references needed
    // to interpret its current thread and rows. Those references may predate since.
    let referenced_turns: std::collections::BTreeSet<_> = old
        .items
        .iter()
        .filter_map(|row| row.turn_id)
        .chain(old.inputs.iter().filter_map(|row| row.turn_id))
        .chain(old.thread.current_turn_id)
        .collect();
    assert!(
        old.turns
            .iter()
            .all(|row| row.seq_updated > before.seq || referenced_turns.contains(&row.id))
    );
    assert!(old.inputs.iter().all(|row| row.seq_updated > before.seq));
    let referenced_segments: std::collections::BTreeSet<_> = old
        .items
        .iter()
        .map(|row| row.segment_id)
        .chain(old.turns.iter().map(|row| row.segment_id))
        .chain(old.thread.current_segment_id)
        .collect();
    assert!(
        old.segments
            .iter()
            .all(|row| row.seq_updated > before.seq || referenced_segments.contains(&row.id))
    );
    assert!(old.seq < last);
    let next = sync(&scene, thread, Some(old.seq)).await;
    bounded(&next);
    assert_eq!(next.since, Some(old.seq));
    assert!(next.seq >= last);
    assert!(
        !next.complete,
        "the next coherent cut explicitly asks for a reload"
    );
    assert!(next.items.is_empty());
    Ok(())
}

#[tokio::test]
async fn opening_outline_and_conversation_keep_the_same_cut_while_the_native_turn_ends() -> Proof {
    let (scene, thread, start) = running().await;
    let read = async {
        scene
            .rig
            .world
            .app
            .call(ThreadOpeningGet, &[&thread])
            .token(&scene.ada.token)
            .query(&ThreadOpeningGetQuery {
                limit: Some(5),
                ..Default::default()
            })
            .send()
            .await
            .ok()
    };
    let (opening, terminal) =
        interleave(Cut::HttpHead, read, complete(&scene, thread, &start)).await?;
    bounded(&opening.sync);
    assert_eq!(opening.outline.seq, opening.sync.seq);
    assert!(
        opening
            .outline
            .entries
            .iter()
            .all(|entry| entry.seq <= opening.sync.seq)
    );
    assert!(opening.sync.seq < terminal);
    assert_eq!(opening.sync.thread.current_turn_id, Some(start.turn_id));
    Ok(())
}

#[tokio::test]
async fn socket_request_cards_share_the_row_cut_and_never_share_another_readers_authority() -> Proof
{
    let (scene, thread, start) = running().await;
    scene
        .rig
        .say(
            thread,
            rig::command_asked(start.turn_id, "read-cut-approval"),
        )
        .await;
    scene.rig.wait_state(thread, "waiting_approval").await;
    let waiting = sync(&scene, thread, None).await;
    let approval = waiting
        .items
        .iter()
        .find_map(|item| match &item.payload {
            ItemPayload::Approval(payload) => Some(payload.approval_id),
            _ => None,
        })
        .ok_or("approval absent")?;
    let inner = socket_inner(&scene);
    let mut cache = Cache::default();
    let card = |rows: &ThreadData| {
        rows.items
            .iter()
            .flatten()
            .find_map(|item| match &item.payload {
                ItemPayload::Approval(payload) if payload.approval_id == approval => {
                    payload.approval.as_deref()
                }
                _ => None,
            })
            .cloned()
    };
    let viewer =
        frame(threads::read(&inner, scene.cai.id(), thread, 0, waiting.seq, &mut cache).await?)?;
    assert!(!card(data(&viewer)?).ok_or("viewer card absent")?.can_decide);
    let decide = async {
        let answer = scene
            .rig
            .world
            .app
            .call(ApprovalDecide, &[&approval])
            .token(&scene.ada.token)
            .body(&ApprovalDecision::Option(HarnessOptionDecision {
                option_id: "allow_once".into(),
                note: None,
            }))
            .send()
            .await
            .ok();
        assert_eq!(answer.state, ApprovalState::Approved);
        Ok(answer)
    };
    let (rows, _) = interleave(
        Cut::SocketRows,
        threads::read(&inner, scene.ada.id(), thread, 0, waiting.seq, &mut cache),
        decide,
    )
    .await?;
    let old = frame(rows?)?;
    let old_data = data(&old)?;
    let old_card = card(old_data).ok_or("owner card absent")?;
    assert_eq!(old_card.state, ApprovalState::Pending);
    assert!(old_card.can_decide);
    assert_eq!(
        old_data
            .thread
            .as_ref()
            .ok_or("waiting thread absent")?
            .state,
        ThreadState::WaitingApproval
    );
    let after = sync(&scene, thread, Some(old.highest)).await;
    let next = frame(
        threads::read(
            &inner,
            scene.ada.id(),
            thread,
            old.highest,
            after.seq,
            &mut cache,
        )
        .await?,
    )?;
    assert_eq!(
        card(data(&next)?).ok_or("decided card absent")?.state,
        ApprovalState::Approved
    );
    Ok(())
}

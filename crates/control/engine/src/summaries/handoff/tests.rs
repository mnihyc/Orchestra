//! Real recorded turns plus explicit reader metadata; no native fork/activation claim.

#[path = "../../../../api/tests/continuation_read_support/mod.rs"]
mod reader;

use super::*;
use crate::test_common as common;
use orch_authz::Caller;
use orch_types::Time;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock, PoisonError},
};
use tokio::sync::oneshot;

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
struct Gate {
    view: Id,
    reached: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}
static GATES: OnceLock<Mutex<HashMap<Id, Gate>>> = OnceLock::new();

pub(super) async fn captured(thread: Id, view: Id) {
    let gate = GATES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&thread);
    if let Some(gate) = gate {
        assert_eq!(view, gate.view);
        let _ = gate.reached.send(());
        let _ = gate.release.await;
    }
}

struct Scene {
    world: common::World,
    thread: Id,
    views: reader::Retained,
}

impl Scene {
    async fn new() -> Proof<Self> {
        let world = common::World::new().await;
        let thread = world.thread("A captured handoff source").await;
        for (number, text, reply) in [
            (0, "KEPT-FIRST-INPUT", "KEPT-FIRST-ANSWER"),
            (1, "EXCLUDED-SECOND-INPUT", "EXCLUDED-SECOND-ANSWER"),
        ] {
            world
                .engine()
                .submit_input(&Caller::session(world.ada), thread, common::queued(text))
                .await?;
            world.run_turn(thread, number, reply).await;
            common::eventually(
                "the actual terminal turn is accounted before retaining a reader",
                || async {
                    let mut conn = match world.pool.acquire().await {
                        Ok(conn) => conn,
                        Err(_) => return false,
                    };
                    !threads::turn_end_work_pending(&mut conn, thread)
                        .await
                        .unwrap_or(true)
                },
            )
            .await;
        }
        let views =
            reader::retained(&world.pool, thread, world.ada, Time::new(world.clock.now())).await?;
        reader::select(
            &world.pool,
            thread,
            views.child,
            Time::new(world.clock.now()),
        )
        .await?;
        let mut tx = world.pool.begin().await?;
        let seq = orch_db::sequences::next(&mut tx, thread)
            .await?
            .ok_or("the actual sequence is absent")?;
        assert_eq!(
            sqlx::query(
                "UPDATE items SET payload=jsonb_set(payload,'{parts}',$3),seq_updated=$4
            WHERE thread_id=$1 AND kind='user_message' AND payload->>'input_id'=$2::uuid::text"
            )
            .bind(thread)
            .bind(views.first_input)
            .bind(json!([{"type":"text","text":"SOURCE-MUTATED-LATER"}]))
            .bind(seq)
            .execute(&mut *tx)
            .await?
            .rows_affected(),
            1
        );
        tx.commit().await?;
        Ok(Self {
            world,
            thread,
            views,
        })
    }

    async fn assert_content(&self, transcript: &IndexedTranscript) -> Proof {
        assert_eq!(transcript.turns, 1);
        let author: String = sqlx::query_scalar("SELECT display_name FROM users WHERE id=$1")
            .bind(self.world.ada)
            .fetch_one(self.world.pool.pool())
            .await?;
        let index = transcript
            .text
            .lines()
            .find(|line| line.starts_with("- Turn 1 ·"))
            .ok_or("the actual first-turn index is absent")?;
        assert!(index.contains(&format!(
            " · {author} · KEPT-FIRST-INPUT · completed · line "
        )));
        assert!(transcript.text.contains("KEPT-FIRST-ANSWER"));
        assert!(!transcript.text.contains("SOURCE-MUTATED-LATER"));
        assert!(!transcript.text.contains("EXCLUDED-SECOND"));
        let revisions: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM thread_item_revisions r JOIN items i ON i.id=r.item_id
            WHERE i.thread_id=$1 AND i.payload->>'input_id'=$2::uuid::text",
        )
        .bind(self.thread)
        .bind(self.views.first_input)
        .fetch_one(self.world.pool.pool())
        .await?;
        assert_eq!(
            revisions, 1,
            "the index must have used the actually retained revision, not a synthetic preview"
        );
        assert!(sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM turns WHERE thread_id=$1 AND input_id=$2 AND ordinal=2)")
            .bind(self.thread).bind(self.views.before_input).fetch_one(self.world.pool.pool()).await?);
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM items WHERE id=$1 AND thread_id=$2
            AND kind='agent_message' AND payload::text LIKE '%KEPT-FIRST-ANSWER%')"
            )
            .bind(self.views.prefix_item)
            .bind(self.thread)
            .fetch_one(self.world.pool.pool())
            .await?
        );
        assert_eq!(self.world.turn_starts(self.thread).len(), 2);
        Ok(())
    }
}

#[tokio::test]
async fn an_index_preview_and_full_transcript_use_the_same_frozen_authored_item_and_turn_metadata()
-> Proof {
    let scene = Scene::new().await?;
    let transcript = indexed_transcript(&scene.world.engine(), scene.thread, None).await?;
    scene.assert_content(&transcript).await?;
    scene.world.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn selection_during_handoff_read_cannot_change_its_captured_history_headers_or_index() -> Proof
{
    let scene = Scene::new().await?;
    let expected = indexed_transcript(&scene.world.engine(), scene.thread, None).await?;
    let (notice, reached) = oneshot::channel();
    let (release, resume) = oneshot::channel();
    assert!(
        GATES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                scene.thread,
                Gate {
                    view: scene.views.child,
                    reached: notice,
                    release: resume
                }
            )
            .is_none()
    );
    let engine = scene.world.engine();
    let thread = scene.thread;
    let reading = tokio::spawn(async move { indexed_transcript(&engine, thread, None).await });
    tokio::time::timeout(common::PATIENCE, reached).await??;
    reader::select(
        &scene.world.pool,
        scene.thread,
        scene.views.root,
        Time::new(scene.world.clock.now()),
    )
    .await?;
    release
        .send(())
        .map_err(|_| "the captured source read ended prematurely")?;
    let transcript = tokio::time::timeout(common::PATIENCE, reading).await???;
    assert_eq!(
        transcript.text, expected.text,
        "the active pointer changed after capture; neither a header nor a turn may recapture it"
    );
    scene.assert_content(&transcript).await?;
    assert_eq!(
        scene
            .world
            .scalar::<Id>(
                "SELECT active_continuation_id FROM threads WHERE node_id=$1",
                scene.thread
            )
            .await,
        scene.views.root
    );
    scene.world.shutdown().await;
    Ok(())
}

#[test]
fn handoff_tail_bounds_count_escaping_and_the_omission_notice_at_every_byte_boundary() {
    let text = "older material & <transcript>\n最新记录🙂 newest end";
    for room in 0..=text.len() * 5 {
        let raw = within(text, room);
        assert!(raw.len() <= room, "raw tail exceeded {room}: {raw:?}");
        if let Some(kept) = model_material_within(text, room) {
            let escaped = orch_summarizer_model::prompts::material(&kept);
            assert!(
                escaped.len() <= room,
                "escaped tail exceeded {room}: {escaped:?}"
            );
            if kept != text {
                assert!(kept.starts_with("(the start is left out)\n"));
                assert!(text.ends_with(kept.trim_start_matches("(the start is left out)\n")));
            }
        }
    }
    assert_eq!(model_material_within(text, 0), None);
    assert_eq!(model_material_within("", 0).as_deref(), Some(""));
}

#[tokio::test]
async fn a_handoff_prefix_cut_cannot_reveal_part_of_a_secret() -> Proof {
    let world = common::World::new().await;
    let thread = world.thread("A redacted handoff").await;
    let secret = "opaque-secret-that-must-remain-a-complete-match";
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            common::queued(&format!("Check {secret} before continuing")),
        )
        .await?;
    world.run_turn(thread, 0, "The check is complete").await;
    // Apply the current rule after the turn was recorded, so the stored item really needs masking now.
    world
        .set(
            world.topic,
            "redaction.patterns",
            json!([{"name":"handoff secret","regex":secret}]),
        )
        .await;
    let mut conn = feeds::begin_reading(&world.pool).await?;
    let row = threads::thread(&mut conn, thread)
        .await?
        .ok_or("the recorded thread is absent")?;
    let raw = written::read_conversation_view(
        &mut conn,
        thread,
        row.active_continuation_id,
        (Some(1), Some(1)),
        Detail::Brief,
        None,
        None,
    )
    .await?
    .ok_or("the recorded turn is absent")?
    .text;
    let start = raw.find(secret).ok_or("the actual brief has no secret")?;
    let room = start + secret.len() / 2;
    let redactor = crate::raw_events::live_redactor(&mut conn, &row, row.environment_id)
        .await?
        .ok_or("the source cannot be redacted")?;
    let masked = crate::raw_events::mask_text(&redactor, raw.clone());
    assert!(raw[..room].contains("opaque-secret"));
    assert!(masked.contains("[masked]"));
    let recent = brief_tail(&mut conn, &row, room)
        .await?
        .ok_or("the recent turn was omitted")?;
    let mut end = room.min(masked.len());
    while !masked.is_char_boundary(end) {
        end -= 1;
    }
    assert_eq!(recent.text, masked[..end]);
    assert!(!recent.text.contains("opaque-secret"));
    assert!(recent.text.len() <= room);
    conn.commit().await?;
    world.shutdown().await;
    Ok(())
}

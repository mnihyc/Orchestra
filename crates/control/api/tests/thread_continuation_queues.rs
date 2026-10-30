//! Inactive pending words keep their own queue and Stop while execution controls cannot target another history.

mod continuation_read_support;
mod places;
mod rig;
mod support;

use continuation_read_support::{Proof, Retained, retained, select};
use orch_api_contract::{
    common::ErrorCode,
    threads::{
        ContentPart, Delivery, ThreadInput, ThreadInputCancel, ThreadInputUpdate,
        ThreadInputUpdateBody, ThreadInterrupt, ThreadInterruptBody, ThreadQueueResume,
        ThreadQueueResumeBody,
    },
};
use orch_types::{Id, Time};

async fn setup() -> Proof<(rig::Scene, Id, Retained, ThreadInput, ThreadInput)> {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene.exchange(thread, 0, "First", "One").await;
    scene.exchange(thread, 1, "Second", "Two").await;
    rig::eventually(
        "the ordinary native turns finish before queue-only admission",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT NOT EXISTS(SELECT 1 FROM turns WHERE thread_id=$1
          AND (end_work IS NOT NULL OR files_end_due OR files_work_pending))",
            )
            .bind(thread)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap_or(false)
        },
    )
    .await;
    scene.rig.engine.stop();
    let old = scene
        .say(
            &scene.ada,
            thread,
            "Retained pending words",
            Delivery::Queue,
            "retained-pending",
        )
        .await
        .ok()
        .input;
    let last: Id =
        sqlx::query_scalar("SELECT id FROM turns WHERE thread_id=$1 ORDER BY ordinal DESC LIMIT 1")
            .bind(thread)
            .fetch_one(scene.rig.db.pool())
            .await?;
    scene
        .rig
        .world
        .app
        .call(ThreadInterrupt, &[&thread])
        .token(&scene.ada.token)
        .body(&ThreadInterruptBody { turn_id: last })
        .send()
        .await
        .ok();
    let view = retained(
        &scene.rig.world.app.app.db,
        thread,
        scene.ada.id(),
        Time::new(scene.rig.world.now()),
    )
    .await?;
    select(
        &scene.rig.world.app.app.db,
        thread,
        view.child,
        Time::new(scene.rig.world.now()),
    )
    .await?;
    let selected = scene
        .say(
            &scene.ada,
            thread,
            "Selected held words",
            Delivery::Queue,
            "selected-pending",
        )
        .await
        .ok()
        .input;
    Ok((scene, thread, view, old, selected))
}

async fn queues(scene: &rig::Scene, thread: Id) -> Proof<Vec<(Id, bool, i64)>> {
    Ok(sqlx::query_as("SELECT id,queue_held,queue_revision FROM thread_continuations WHERE thread_id=$1 ORDER BY id")
        .bind(thread).fetch_all(scene.rig.db.pool()).await?)
}

#[tokio::test]
async fn inactive_delivery_and_stop_refuse_but_author_text_edits_and_cancellation_leave_the_selected_hold_alone()
-> Proof {
    let (scene, thread, view, old, selected) = setup().await?;
    let app = &scene.rig.world.app;
    let before = queues(&scene, thread).await?;
    let native = scene.rig.commands.issued_total();
    for body in [
        ThreadInputUpdateBody {
            delivery: Some(Delivery::Interrupt),
            ..Default::default()
        },
        ThreadInputUpdateBody {
            delivery: Some(Delivery::Steer),
            ..Default::default()
        },
        ThreadInputUpdateBody {
            after_input_id: Some(None),
            ..Default::default()
        },
    ] {
        let refused = app
            .call(ThreadInputUpdate, &[&thread, &old.id])
            .token(&scene.ada.token)
            .body(&body)
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::ConflictStaleRevision);
        assert_eq!(
            refused.extra.get("continuation_id"),
            Some(&serde_json::json!(view.root))
        );
    }
    let original_turn: Id = sqlx::query_scalar("SELECT id FROM turns WHERE input_id=$1")
        .bind(view.before_input)
        .fetch_one(scene.rig.db.pool())
        .await?;
    let stopped = app
        .call(ThreadInterrupt, &[&thread])
        .token(&scene.ada.token)
        .body(&ThreadInterruptBody {
            turn_id: original_turn,
        })
        .send()
        .await
        .problem();
    assert_eq!(stopped.code, ErrorCode::ThreadNotRunning);
    assert_eq!(queues(&scene, thread).await?, before);
    let edit = app
        .call(ThreadInputUpdate, &[&thread, &old.id])
        .token(&scene.ada.token)
        .body(&ThreadInputUpdateBody {
            content: Some(vec![ContentPart::Text {
                text: "Revised pending words".into(),
            }]),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(edit.input.continuation_id, Some(view.root));
    assert!(edit.input.queue_held);
    assert_eq!(
        edit.input.content,
        vec![ContentPart::Text {
            text: "Revised pending words".into()
        }]
    );
    let edited = queues(&scene, thread).await?;
    assert_eq!(
        edited.iter().find(|row| row.0 == view.child),
        before.iter().find(|row| row.0 == view.child)
    );
    let denied = app
        .call(ThreadInputUpdate, &[&thread, &old.id])
        .token(&scene.dan.token)
        .body(&ThreadInputUpdateBody {
            content: Some(vec![ContentPart::Text {
                text: "Borrowed author words".into(),
            }]),
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    app.call(ThreadInputCancel, &[&thread, &old.id])
        .token(&scene.ada.token)
        .send()
        .await
        .ok();
    let after = queues(&scene, thread).await?;
    assert_eq!(
        after.iter().find(|row| row.0 == view.child),
        before.iter().find(|row| row.0 == view.child)
    );
    assert!(
        !after
            .iter()
            .find(|row| row.0 == view.root)
            .ok_or("retained queue disappeared")?
            .1
    );
    let kept: (Id, String, bool) =
        sqlx::query_as("SELECT continuation_id,state,queue_held FROM thread_inputs WHERE id=$1")
            .bind(selected.id)
            .fetch_one(scene.rig.db.pool())
            .await?;
    assert_eq!(kept, (view.child, "pending".into(), true));
    assert_eq!(scene.rig.commands.issued_total(), native);
    assert_eq!(scene.rig.turn_starts(thread).len(), 2);
    Ok(())
}

#[tokio::test]
async fn equal_retained_queue_numbers_and_an_away_back_selection_cannot_reuse_a_delayed_resume()
-> Proof {
    let (scene, thread, view, old, selected) = setup().await?;
    let app = &scene.rig.world.app;
    let revision: i64 = sqlx::query_scalar("SELECT queue_revision FROM threads WHERE node_id=$1")
        .bind(thread)
        .fetch_one(scene.rig.db.pool())
        .await?;
    // Reconstruct the collision explicitly: the inactive queue can independently reach this same number.
    sqlx::query("UPDATE thread_continuations SET queue_revision=$2 WHERE id=$1")
        .bind(view.root)
        .bind(revision)
        .execute(scene.rig.db.pool())
        .await?;
    select(
        &scene.rig.world.app.app.db,
        thread,
        view.root,
        Time::new(scene.rig.world.now()),
    )
    .await?;
    select(
        &scene.rig.world.app.app.db,
        thread,
        view.child,
        Time::new(scene.rig.world.now()),
    )
    .await?;
    let new_revision: i64 =
        sqlx::query_scalar("SELECT queue_revision FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_one(scene.rig.db.pool())
            .await?;
    assert!(new_revision > revision);
    let before = queues(&scene, thread).await?;
    let native = scene.rig.commands.issued_total();
    let refused = app
        .call(ThreadQueueResume, &[&thread])
        .token(&scene.ada.token)
        .body(&ThreadQueueResumeBody { revision })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(queues(&scene, thread).await?, before);
    app.call(ThreadQueueResume, &[&thread])
        .token(&scene.ada.token)
        .body(&ThreadQueueResumeBody {
            revision: new_revision,
        })
        .send()
        .await
        .ok();
    let inputs: Vec<(Id, Id, String, bool)> = sqlx::query_as(
        "SELECT id,continuation_id,state,queue_held FROM thread_inputs
      WHERE id=ANY($1) ORDER BY id",
    )
    .bind(vec![old.id, selected.id])
    .fetch_all(scene.rig.db.pool())
    .await?;
    assert_eq!(inputs.len(), 2);
    assert_eq!(
        inputs
            .iter()
            .find(|row| row.0 == old.id)
            .map(|row| (row.1, row.2.as_str(), row.3)),
        Some((view.root, "pending", true))
    );
    assert_eq!(
        inputs
            .iter()
            .find(|row| row.0 == selected.id)
            .map(|row| (row.1, row.2.as_str(), row.3)),
        Some((view.child, "pending", false))
    );
    assert_eq!(scene.rig.commands.issued_total(), native);
    assert_eq!(scene.rig.turn_starts(thread).len(), 2);
    Ok(())
}

//! Real HTTP future inputs keep drafts private, use current authority and queue each occurrence once.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use chrono::Duration;
use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::input_schedules::*;
use orch_api_contract::nodes::{CreateThread, CreateThreadBinding};
use orch_api_contract::threads::ContentPart;
use orch_types::{Id, Time};
use serde_json::json;

fn once(scene: &rig::Scene, target: InputScheduleTarget, minutes: i64) -> InputScheduleCreateBody {
    InputScheduleCreateBody {
        name: Some("Private prepared input".to_owned()),
        target,
        content: vec![ContentPart::Text {
            text: "the author's private future prompt".to_owned(),
        }],
        native_action: None,
        timing: InputTiming::Once {
            at: Time::new(scene.rig.world.now() + Duration::minutes(minutes)),
            zone: "UTC".to_owned(),
        },
    }
}

async fn get(scene: &rig::Scene, id: Id) -> InputSchedule {
    scene
        .rig
        .world
        .app
        .call(InputScheduleGet, &[&id])
        .token(&scene.ada.token)
        .send()
        .await
        .ok()
}

async fn offline(scene: &rig::Scene) {
    sqlx::query("UPDATE machines SET online=false WHERE id=$1")
        .bind(scene.rig.machine_id)
        .execute(scene.rig.db.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn authored_edits_controls_and_once_execution_have_real_safe_deduplicated_history() {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await;
    offline(&scene).await;
    let body = once(
        &scene,
        InputScheduleTarget::Thread {
            thread_id: thread.base.id,
        },
        2,
    );
    let created = scene
        .rig
        .world
        .app
        .call(TopicInputScheduleCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(created.state, InputScheduleState::Active);
    assert!(created.definition.is_some());
    let turns: i64 = sqlx::query_scalar("SELECT count(*) FROM turns WHERE thread_id=$1")
        .bind(thread.base.id)
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(turns, 0, "saving future work starts no model turn");
    let metadata = scene
        .rig
        .world
        .app
        .call(InputScheduleGet, &[&created.id])
        .token(&scene.dan.token)
        .send()
        .await
        .ok();
    assert!(
        metadata.definition.is_none(),
        "a destination reader sees metadata, never the authored draft"
    );
    assert!(
        !serde_json::to_string(&metadata)
            .unwrap()
            .contains("private future prompt")
    );
    let changed = scene
        .rig
        .world
        .app
        .call(InputScheduleUpdate, &[&created.id])
        .token(&scene.ada.token)
        .body(&InputScheduleUpdateBody {
            revision: created.revision,
            name: Some("Changed future input".to_owned()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let paused = scene
        .rig
        .world
        .app
        .call(InputScheduleUpdate, &[&created.id])
        .token(&scene.ada.token)
        .body(&InputScheduleUpdateBody {
            revision: changed.revision,
            state: Some(InputScheduleState::Paused),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(paused.next_run_at.is_none());
    assert_eq!(
        orch_api_core::input_schedules::run_due(&scene.rig.world.app.app)
            .await
            .unwrap(),
        0
    );
    let resumed = scene
        .rig
        .world
        .app
        .call(InputScheduleUpdate, &[&created.id])
        .token(&scene.ada.token)
        .body(&InputScheduleUpdateBody {
            revision: paused.revision,
            state: Some(InputScheduleState::Active),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let stale = scene
        .rig
        .world
        .app
        .call(InputScheduleUpdate, &[&created.id])
        .token(&scene.ada.token)
        .body(&InputScheduleUpdateBody {
            revision: paused.revision,
            state: Some(InputScheduleState::Paused),
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    scene.rig.world.advance(Duration::minutes(3));
    // While another process's pass holds the lease this one leaves the due input to it.
    let other = orch_db::queries::job_leases::Lease::new(
        "input_schedules",
        "another process",
        Duration::minutes(2),
    );
    assert!(
        other
            .hold(&scene.rig.db, scene.rig.world.now())
            .await
            .unwrap()
    );
    assert_eq!(
        orch_api_core::input_schedules::run_due(&scene.rig.world.app.app)
            .await
            .unwrap(),
        0
    );
    other.release(&scene.rig.db).await.unwrap();
    assert_eq!(
        orch_api_core::input_schedules::run_due(&scene.rig.world.app.app)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        orch_api_core::input_schedules::run_due(&scene.rig.world.app.app)
            .await
            .unwrap(),
        0
    );
    let finished = get(&scene, created.id).await;
    assert_eq!(finished.state, InputScheduleState::Finished);
    assert!(finished.next_run_at.is_none());
    let run = finished.last_run.unwrap();
    assert!(run.input_id.is_some());
    let prompt: serde_json::Value =
        sqlx::query_scalar("SELECT content FROM thread_inputs WHERE id=$1")
            .bind(run.input_id)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(prompt, json!(body.content));
    let late = scene
        .rig
        .world
        .app
        .call(InputScheduleUpdate, &[&created.id])
        .token(&scene.ada.token)
        .body(&InputScheduleUpdateBody {
            revision: finished.revision,
            state: Some(InputScheduleState::Active),
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(
        late.code,
        ErrorCode::RequestInvalid,
        "a consumed once time needs a new future time"
    );
    scene
        .rig
        .world
        .app
        .call(InputScheduleDelete, &[&created.id])
        .token(&scene.ada.token)
        .send()
        .await
        .ok();
    scene
        .rig
        .world
        .app
        .call(InputScheduleDelete, &[&created.id])
        .token(&scene.ada.token)
        .send()
        .await
        .ok();
    let input_kept: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_inputs WHERE id=$1)")
            .bind(run.input_id)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap();
    assert!(
        input_kept,
        "removing future work does not cancel its accepted input"
    );
    let events: Vec<(String, Option<Id>, serde_json::Value)> = sqlx::query_as(
        "SELECT action,actor_id,details FROM audit_log WHERE object_id=$1 ORDER BY at,id",
    )
    .bind(created.id)
    .fetch_all(scene.rig.db.pool())
    .await
    .unwrap();
    for action in [
        "input_schedule.created",
        "input_schedule.updated",
        "input_schedule.paused",
        "input_schedule.resumed",
        "input_schedule.deleted",
        "input_schedule.ran",
    ] {
        let rows: Vec<_> = events.iter().filter(|row| row.0 == action).collect();
        assert_eq!(rows.len(), 1, "{action}");
        assert_eq!(rows[0].1, Some(scene.ada.id()));
        assert!(!rows[0].2.to_string().contains("private future prompt"));
    }
    assert!(resumed.revision > changed.revision);
}

#[tokio::test]
async fn a_due_occurrence_loses_current_authority_instead_of_reusing_creation_and_tells_its_author_once()
 {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await;
    offline(&scene).await;
    let created = scene
        .rig
        .world
        .app
        .call(TopicInputScheduleCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&once(
            &scene,
            InputScheduleTarget::Thread {
                thread_id: thread.base.id,
            },
            1,
        ))
        .send()
        .await
        .ok();
    sqlx::query("UPDATE users SET status='suspended' WHERE id=$1")
        .bind(scene.ada.id())
        .execute(scene.rig.db.pool())
        .await
        .unwrap();
    scene.rig.world.advance(Duration::minutes(2));
    assert_eq!(
        orch_api_core::input_schedules::run_due(&scene.rig.world.app.app)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        orch_api_core::input_schedules::run_due(&scene.rig.world.app.app)
            .await
            .unwrap(),
        0
    );
    sqlx::query("UPDATE users SET status='active' WHERE id=$1")
        .bind(scene.ada.id())
        .execute(scene.rig.db.pool())
        .await
        .unwrap();
    let finished = get(&scene, created.id).await;
    let run = finished.last_run.unwrap();
    assert_eq!(run.state, InputScheduleRunState::Failed);
    assert!(run.input_id.is_none());
    let inputs: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
        .bind(thread.base.id)
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(inputs, 0);
    let records: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE object_id=$1 AND action='input_schedule.cannot_start'",
    )
    .bind(created.id)
    .fetch_all(scene.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["run"], json!(run.id));
    assert!(!records[0].to_string().contains("private future prompt"));
    let notices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id=$1 AND kind='schedule.cannot_start'",
    )
    .bind(scene.ada.id())
    .fetch_one(scene.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(notices, 1);
}

#[tokio::test]
async fn new_thread_choices_are_resolved_when_due_and_creation_is_atomic_with_its_first_input() {
    let scene = rig::Scene::new().await;
    offline(&scene).await;
    let config = CreateThread {
        binding: Some(CreateThreadBinding {
            agent_id: scene.on.agent,
            provider_id: scene.on.provider,
            arrangement: None,
        }),
        model: Some(rig::MODEL.to_owned()),
        environment: Some(EnvironmentChoice::Use(scene.on.environment)),
        ..Default::default()
    };
    let created = scene
        .rig
        .world
        .app
        .call(TopicInputScheduleCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&once(
            &scene,
            InputScheduleTarget::NewThread {
                config: Box::new(config),
            },
            1,
        ))
        .send()
        .await
        .ok();
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM threads WHERE topic_id=$1")
        .bind(scene.tree.topic.base.id)
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        before, 0,
        "a creation check rolls back its trial thread and turn"
    );
    scene.rig.world.advance(Duration::minutes(2));
    orch_api_core::input_schedules::run_due(&scene.rig.world.app.app)
        .await
        .unwrap();
    orch_api_core::input_schedules::run_due(&scene.rig.world.app.app)
        .await
        .unwrap();
    let finished = get(&scene, created.id).await;
    let run = finished.last_run.unwrap();
    assert!(run.thread.is_some() && run.input_id.is_some());
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM threads WHERE topic_id=$1")
        .bind(scene.tree.topic.base.id)
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        rows, 1,
        "lost answers and a second sweep cannot create another thread"
    );
    assert!(
        run.thread
            .unwrap()
            .name
            .unwrap()
            .contains("Private prepared input")
    );
}

#[tokio::test]
async fn tokens_cannot_create_edit_or_resume_future_authority_but_can_pause_and_remove_it() {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await;
    let body = once(
        &scene,
        InputScheduleTarget::Thread {
            thread_id: thread.base.id,
        },
        5,
    );
    let schedule = scene
        .rig
        .world
        .app
        .call(TopicInputScheduleCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    let token = scene
        .rig
        .world
        .app
        .call(orch_api_contract::auth::MeTokenCreate, &[])
        .token(&scene.ada.token)
        .body(&orch_api_contract::auth::ApiTokenCreateBody {
            name: "Future-work control".to_owned(),
            scopes: Some(vec!["*".to_owned()]),
            node_id: None,
            expires_at: None,
            proof: orch_api_contract::auth::SensitiveProof {
                password: Some(support::PASSWORD.to_owned()),
                ..Default::default()
            },
        })
        .send()
        .await
        .ok()
        .token
        .unwrap();
    let refused = scene
        .rig
        .world
        .app
        .call(TopicInputScheduleCreate, &[&scene.tree.topic.base.id])
        .token(&token)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthSessionRequired);
    let refused = scene
        .rig
        .world
        .app
        .call(InputScheduleUpdate, &[&schedule.id])
        .token(&token)
        .body(&InputScheduleUpdateBody {
            revision: schedule.revision,
            name: Some("A token's draft".to_owned()),
            state: Some(InputScheduleState::Paused),
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthSessionRequired);
    let paused = scene
        .rig
        .world
        .app
        .call(InputScheduleUpdate, &[&schedule.id])
        .token(&token)
        .body(&InputScheduleUpdateBody {
            revision: schedule.revision,
            state: Some(InputScheduleState::Paused),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let refused = scene
        .rig
        .world
        .app
        .call(InputScheduleUpdate, &[&schedule.id])
        .token(&token)
        .body(&InputScheduleUpdateBody {
            revision: paused.revision,
            state: Some(InputScheduleState::Active),
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthSessionRequired);
    scene
        .rig
        .world
        .app
        .call(InputScheduleDelete, &[&schedule.id])
        .token(&token)
        .send()
        .await
        .ok();
}

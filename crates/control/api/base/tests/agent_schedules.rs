//! Managed agent wakeups use real admitted tool sources, existing schedule claims and ordinary inputs.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#[path = "agent_schedules/held.rs"]
mod held;
mod places;
mod rig;
mod rig_tool_source;
mod support;
use chrono::Duration;
use orch_agent_contract::events::OrchTool;
use orch_api_contract::common::PageQuery;
use orch_api_contract::input_schedules::{
    InputSchedule, InputScheduleDelete, InputScheduleGet, InputScheduleKind, InputScheduleRunList,
    InputScheduleState,
};
use orch_engine::commands::Commands;
use orch_engine::tools::CallContext;
use orch_link_contract::events::TurnOutcome;
use orch_settings::Target;
use orch_types::{Id, Json, new_id};
use serde_json::json;

async fn setup(enabled: bool) -> (rig::Scene, Id, Id) {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    if enabled {
        scene
            .rig
            .world
            .set_at(Target::Node(thread), "threads.agent_schedules", json!("on"))
            .await;
    }
    let turn = rig_tool_source::active(&scene, thread).await.unwrap();
    (scene, thread, turn)
}

async fn call(scene: &rig::Scene, thread: Id, turn: Id, reference: &str, arguments: Json) -> Json {
    scene
        .rig
        .engine
        .answer_tool_call(&CallContext {
            machine: scene.rig.machine_id,
            thread,
            turn,
            call_ref: reference.into(),
            attempt: None,
            tool: OrchTool::Schedule,
            arguments,
        })
        .await
        .unwrap();
    scene
        .rig
        .commands
        .retained_body(orch_engine::ids::tool_result(thread, reference))
        .await
        .unwrap()
        .expect("the same native command port retained the canonical answer")
}

async fn create(scene: &rig::Scene, thread: Id, turn: Id, reference: &str, arguments: Json) -> Id {
    let reply = call(scene, thread, turn, reference, arguments).await;
    assert!(reply["error"].is_null(), "{reply}");
    serde_json::from_value(reply["result"]["id"].clone()).unwrap()
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

async fn finish(scene: &rig::Scene, thread: Id, turn: Id) {
    scene
        .rig
        .say(thread, rig::turn_ended(turn, TurnOutcome::Completed))
        .await;
    scene.rig.wait_turn(turn, "completed").await;
    scene.rig.wait_state(thread, "idle").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_default_off_tool_cannot_create_work_and_a_claimed_source_cannot_name_another_thread() {
    let (scene, thread, turn) = setup(false).await;
    let refused = call(
        &scene,
        thread,
        turn,
        "disabled",
        json!({"in_seconds":60,"note":"Check the requested result"}),
    )
    .await;
    assert!(refused["error"].is_object(), "{refused}");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM input_schedules")
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    scene
        .rig
        .world
        .set_at(Target::Node(thread), "threads.agent_schedules", json!("on"))
        .await;
    let refused = call(
        &scene,
        thread,
        turn,
        "retarget",
        json!({"in_seconds":60,"note":"Check the requested result","thread_id":new_id()}),
    )
    .await;
    assert!(
        refused["error"].is_object(),
        "a closed schema refuses a model-selected target: {refused}"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM input_schedules")
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_native_once_schedule_is_visible_idempotent_and_fires_as_one_ordinary_admitted_input() {
    let (scene, thread, turn) = setup(true).await;
    let args = json!({"in_seconds":60,"note":"Check the CI result requested by the person"});
    let id = create(&scene, thread, turn, "once", args.clone()).await;
    assert_eq!(
        create(&scene, thread, turn, "once", args).await,
        id,
        "replayed native delivery creates no second schedule"
    );
    let row = get(&scene, id).await;
    assert_eq!(row.kind, Some(InputScheduleKind::Agent));
    assert_eq!(row.author.id, scene.ada.id());
    let agent = row.agent.unwrap();
    assert_eq!(
        (agent.thread_id, agent.agent_id, agent.source_turn_id),
        (thread, scene.on.agent, turn)
    );
    assert_eq!((agent.max_runs, agent.runs), (1, 0));
    assert_eq!(
        scene.rig.turn_starts(thread).len(),
        1,
        "saving a future input starts no turn"
    );
    finish(&scene, thread, turn).await;
    scene.rig.world.advance(Duration::seconds(61));
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
    let next = scene.rig.turn_start(thread, 1).await;
    assert!(
        next.text
            .contains("Check the CI result requested by the person")
    );
    let row = get(&scene, id).await;
    assert_eq!(row.state, InputScheduleState::Finished);
    let input:Option<String>=sqlx::query_scalar("SELECT client_id FROM thread_inputs WHERE thread_id=$1 AND client_id LIKE 'agent-schedule:%'")
        .bind(thread).fetch_one(scene.rig.db.pool()).await.unwrap();
    assert!(input.unwrap().starts_with(&format!("agent-schedule:{id}:")));
    let (created,for_user):(i64,Option<Id>)=sqlx::query_as("SELECT count(*),min(for_user_id::text)::uuid FROM audit_log WHERE action='schedule.created_by_agent' AND object_id=$1")
        .bind(id).fetch_one(scene.rig.db.pool()).await.unwrap();
    assert_eq!((created, for_user), (1, Some(scene.ada.id())));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recurring_agent_inputs_use_explicit_ends_instead_of_capping_them_to_the_defaults() {
    let (scene, thread, turn) = setup(true).await;
    scene
        .rig
        .world
        .set_at(
            Target::Node(thread),
            "threads.agent_schedule_hours",
            json!(1),
        )
        .await;
    scene
        .rig
        .world
        .set_at(
            Target::Node(thread),
            "threads.agent_schedule_runs",
            json!(2),
        )
        .await;
    let until = scene.rig.world.now() + Duration::hours(48);
    let explicit=create(&scene,thread,turn,"explicit",json!({"cron":"0 12 * * *","until":until,"max_runs":25,"note":"A longer requested follow-up"})).await;
    let info = get(&scene, explicit).await.agent.unwrap();
    assert_eq!((info.until.get(), info.max_runs), (until, 25));
    let cancelled = call(
        &scene,
        thread,
        turn,
        "cancel-long",
        json!({"command":"cancel","id":explicit}),
    )
    .await;
    assert!(cancelled["error"].is_null(), "{cancelled}");
    let id = create(
        &scene,
        thread,
        turn,
        "recurring",
        json!({"cron":"* * * * *","note":"Check the requested task again"}),
    )
    .await;
    finish(&scene, thread, turn).await;
    for nth in 1..=2 {
        scene.rig.world.advance(Duration::seconds(61));
        assert_eq!(
            orch_api_core::input_schedules::run_due(&scene.rig.world.app.app)
                .await
                .unwrap(),
            1
        );
        scene
            .rig
            .run_turn(thread, nth, "The scheduled check ran")
            .await;
        assert_eq!(
            get(&scene, id).await.agent.unwrap().runs,
            i64::try_from(nth).unwrap()
        );
    }
    assert_eq!(get(&scene, id).await.state, InputScheduleState::Finished);
    scene.rig.world.advance(Duration::minutes(2));
    assert_eq!(
        orch_api_core::input_schedules::run_due(&scene.rig.world.app.app)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_stops_future_agent_inputs_through_both_existing_controller_and_native_tool() {
    let (scene, thread, turn) = setup(true).await;
    let first = create(
        &scene,
        thread,
        turn,
        "cancel-one",
        json!({"in_seconds":60,"note":"Future work to cancel"}),
    )
    .await;
    let result = call(
        &scene,
        thread,
        turn,
        "cancel",
        json!({"command":"cancel","id":first}),
    )
    .await;
    assert!(result["error"].is_null());
    assert_eq!(result["result"]["state"], "deleted");
    let second = create(
        &scene,
        thread,
        turn,
        "cancel-two",
        json!({"in_seconds":60,"note":"Another future input"}),
    )
    .await;
    scene
        .rig
        .world
        .app
        .call(InputScheduleDelete, &[&second])
        .token(&scene.ada.token)
        .send()
        .await
        .ok();
    let listed = call(&scene, thread, turn, "list", json!({"command":"list"})).await;
    assert_eq!(listed["result"]["items"], json!([]));
    scene.rig.world.advance(Duration::minutes(2));
    assert_eq!(
        orch_api_core::input_schedules::run_due(&scene.rig.world.app.app)
            .await
            .unwrap(),
        0
    );
    let cancelled: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='schedule.cancelled' AND object_id=ANY($1)",
    )
    .bind(vec![first, second])
    .fetch_one(scene.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(cancelled, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_owned_agent_can_reduce_future_work_after_the_starter_loses_access() {
    let (scene, thread, turn) = setup(true).await;
    let id = create(
        &scene,
        thread,
        turn,
        "owned-future",
        json!({"in_seconds":60,"note":"Future work whose authority can be lost"}),
    )
    .await;
    // Freeze the supervisor, not the recorded native source, so this checks the
    // cancellation boundary itself rather than racing turn-stop housekeeping.
    scene.rig.engine.stop();
    sqlx::query("UPDATE users SET status='suspended' WHERE id=$1")
        .bind(scene.ada.id())
        .execute(scene.rig.db.pool())
        .await
        .unwrap();
    let cancelled = call(
        &scene,
        thread,
        turn,
        "reduce-owned-work",
        json!({"command":"cancel","id":id}),
    )
    .await;
    assert!(cancelled["error"].is_null(), "{cancelled}");
    assert_eq!(cancelled["result"]["state"], "deleted");
    let creations: i64 =
        sqlx::query_scalar("SELECT count(*) FROM input_schedule_runs WHERE schedule_id=$1")
            .bind(id)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        creations, 0,
        "cancellation acquires no future input authority"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_due_agent_input_records_current_permission_and_provider_allowance_refusals_without_notification()
 {
    for permission_loss in [true, false] {
        let (scene, thread, turn) = setup(true).await;
        let id = create(
            &scene,
            thread,
            turn,
            "current-authority",
            json!({"in_seconds":60,"note":"Do only the requested future work"}),
        )
        .await;
        finish(&scene, thread, turn).await;
        if permission_loss {
            sqlx::query("UPDATE users SET status='suspended' WHERE id=$1")
                .bind(scene.ada.id())
                .execute(scene.rig.db.pool())
                .await
                .unwrap();
        } else {
            sqlx::query("INSERT INTO usage_caps(id,provider_id,meter,period,amount,created_by) VALUES($1,$2,'tokens','day',0,$3)")
                .bind(new_id()).bind(scene.on.provider).bind(scene.ada.id()).execute(scene.rig.db.pool()).await.unwrap();
        }
        scene.rig.world.advance(Duration::seconds(61));
        assert_eq!(
            orch_api_core::input_schedules::run_due(&scene.rig.world.app.app)
                .await
                .unwrap(),
            1
        );
        let (state, reason): (String, Option<String>) =
            sqlx::query_as("SELECT state,reason FROM input_schedule_runs WHERE schedule_id=$1")
                .bind(id)
                .fetch_one(scene.rig.db.pool())
                .await
                .unwrap();
        assert!(
            matches!(state.as_str(), "failed" | "blocked"),
            "{state}: {reason:?}"
        );
        assert!(reason.as_deref().is_some_and(|reason| !reason.is_empty()));
        assert_eq!(
            scene.rig.turn_starts(thread).len(),
            1,
            "refused future work issues no new native turn"
        );
        let notices: i64 =
            sqlx::query_scalar("SELECT count(*) FROM notifications WHERE data->>'schedule'=$1")
                .bind(id.to_string())
                .fetch_one(scene.rig.db.pool())
                .await
                .unwrap();
        assert_eq!(
            notices, 0,
            "agent schedule failures are visible in runs/audit without a default notification"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_recurring_schedule_does_not_catch_up_after_its_end() {
    let (scene, thread, turn) = setup(true).await;
    let id=create(&scene,thread,turn,"end-time",json!({"cron":"* * * * *","until":scene.rig.world.now()+Duration::minutes(2),"max_runs":50,"note":"A short requested watch"})).await;
    finish(&scene, thread, turn).await;
    scene.rig.world.advance(Duration::minutes(3));
    assert_eq!(
        orch_api_core::input_schedules::run_due(&scene.rig.world.app.app)
            .await
            .unwrap(),
        1
    );
    let row = get(&scene, id).await;
    assert_eq!(row.state, InputScheduleState::Finished);
    let runs = scene
        .rig
        .world
        .app
        .call(InputScheduleRunList, &[&id])
        .token(&scene.ada.token)
        .query(&PageQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(runs.items.len(), 1);
    assert_eq!(runs.items[0].reason.as_deref(), Some("schedule_ended"));
    assert!(runs.items[0].input_id.is_none());
    assert_eq!(scene.rig.turn_starts(thread).len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_schedule_keeps_the_actual_starters_key_deadline_instead_of_minting_a_session() {
    use orch_api_contract::threads::{
        ContentPart, Delivery, ThreadInputCreate, ThreadInputCreateBody,
    };
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity;
    let (scene, thread, first) = setup(true).await;
    finish(&scene, thread, first).await;
    let scope: orch_access_contract::key_scope::KeyScope =
        serde_json::from_value(json!({"rules":[{
        "target":{"kind":"thread","id":thread,"reach":"self"},"resources":["thread"],
        "allow":[{"operation":"ThreadInputCreate","actions":["write"]}]}]}))
        .unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut scene.rig.db.acquire().await.unwrap(),
        &identity::NewApiToken {
            id: new_id(),
            user_id: scene.ada.id(),
            name: "Actual native schedule source",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: scene.rig.world.now(),
            expires_at: Some(scene.rig.world.now() + Duration::seconds(30)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    let accepted = scene
        .rig
        .world
        .app
        .call(ThreadInputCreate, &[&thread])
        .token(&secret)
        .body(&ThreadInputCreateBody {
            content: vec![ContentPart::Text {
                text: "Schedule the requested follow-up".into(),
            }],
            delivery: Delivery::Queue,
            client_id: "actual-scoped-schedule-source".into(),
            bypass: None,
            native_action: None,
            plan: false,
        })
        .send_bytes()
        .await;
    assert_eq!(
        accepted.status,
        201,
        "{}",
        String::from_utf8_lossy(&accepted.raw)
    );
    let start = scene.rig.turn_start(thread, 1).await;
    scene
        .rig
        .answer(
            thread,
            &start,
            "scoped-source",
            "Working on the bounded request",
        )
        .await;
    scene.rig.wait_turn(start.turn_id, "running").await;
    let id = create(
        &scene,
        thread,
        start.turn_id,
        "bounded-follow-up",
        json!({"in_seconds":60,"note":"Continue only with the accepted authority"}),
    )
    .await;
    finish(&scene, thread, start.turn_id).await;
    scene.rig.world.advance(Duration::seconds(61));
    assert_eq!(
        orch_api_core::input_schedules::run_due(&scene.rig.world.app.app)
            .await
            .unwrap(),
        1
    );
    let (state, input): (String, Option<Id>) =
        sqlx::query_as("SELECT state,input_id FROM input_schedule_runs WHERE schedule_id=$1")
            .bind(id)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(state, "failed");
    assert!(input.is_none());
    assert_eq!(
        scene.rig.turn_starts(thread).len(),
        2,
        "an expired accepted key cannot become a starter session for future work"
    );
}

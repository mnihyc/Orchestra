//! A confirmed native loss uses the same durable rule, original authority and repeat guard.
use super::{Caller, Id, Json, World, eventually, queued, recovery, turn_began};
use orch_api_contract::calls::CallRecoveryAction;
use orch_link_contract::events::{Event, SessionLost};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_old_physical_sessions_opened_and_lost_reports_cannot_change_its_replacement() {
    use orch_db::queries::link::{NewCommand, insert_command};
    use orch_link_contract::events::SessionOpened;
    let world = World::new().await;
    world.set(world.topic, "calls.repeat_limit", json!(0)).await;
    let thread = world.thread("Only this physical session").await;
    let turn = start(&world, thread, world.ada).await;
    let segment = world.segment_of(thread).await;
    let old = world.opening_of(thread).await;
    let replacement = orch_types::new_id();
    let original = world
        .commands
        .issued_commands("session.open")
        .into_iter()
        .find(|(_, command)| command.id == old)
        .unwrap()
        .1;
    let mut conn = world.pool.acquire().await.unwrap();
    insert_command(
        &mut conn,
        &NewCommand {
            native_target: None,
            id: replacement,
            machine_id: world.machine_id,
            kind: "session.open",
            body: &original.body,
            serial_key: None,
            deadline_at: world.clock.now() + chrono::Duration::minutes(2),
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE segments SET native_session_ref=NULL WHERE id=$1")
        .bind(segment)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    world
        .say_session_taken_in(
            thread,
            Event::SessionOpened(SessionOpened {
                opening_id: old,
                native_session_ref: "old physical conversation".into(),
                resumed: true,
            }),
        )
        .await;
    world
        .say_session_taken_in(
            thread,
            Event::SessionLost(SessionLost {
                uncertain: false,
                expected: false,
                opening_id: old,
                reason: "the earlier process finally reported its exit".into(),
            }),
        )
        .await;
    assert_eq!(world.state(thread).await.0, "running");
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM model_call_recovery WHERE node_id=$1",
                thread
            )
            .await,
        0
    );
    assert!(
        world
            .scalar::<Option<String>>(
                "SELECT native_session_ref FROM segments WHERE id=$1",
                segment
            )
            .await
            .is_none()
    );
    let facts: Json = world
        .scalar(
            "SELECT delivered_resources->'tool_admission' FROM segments WHERE id=$1",
            segment,
        )
        .await;
    assert_eq!(facts["opening_id"], json!(replacement));
    assert_eq!(facts["live"], json!(true));
    world
        .say_session_taken_in(
            thread,
            Event::SessionOpened(SessionOpened {
                opening_id: replacement,
                native_session_ref: "replacement physical conversation".into(),
                resumed: false,
            }),
        )
        .await;
    assert_eq!(
        world
            .scalar::<String>(
                "SELECT native_session_ref FROM segments WHERE id=$1",
                segment
            )
            .await,
        "replacement physical conversation"
    );
    world
        .say_session_taken_in(
            thread,
            Event::SessionLost(SessionLost {
                uncertain: false,
                expected: false,
                opening_id: replacement,
                reason: "the actual current process ended".into(),
            }),
        )
        .await;
    world.wait_turn(turn, "failed").await;
    let facts: Json = world
        .scalar(
            "SELECT delivered_resources->'tool_admission' FROM segments WHERE id=$1",
            segment,
        )
        .await;
    assert_eq!(
        facts["live"],
        json!(false),
        "a confirmed current loss withdraws its tool admission in the same commit"
    );
    let late = serde_json::json!({"native_session_ref":"replacement physical conversation"});
    orch_db::queries::link::finish_command(
        &mut world.pool.acquire().await.unwrap(),
        replacement,
        &orch_db::queries::link::Ending::Succeeded(&late),
        world.clock.now(),
    )
    .await
    .unwrap();
    let delivered: Json = world
        .scalar(
            "SELECT delivered_resources FROM segments WHERE id=$1",
            segment,
        )
        .await;
    assert_eq!(
        delivered["state"],
        json!("closed"),
        "a late acknowledgement cannot undo a physical loss fact"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_connector_restart_retires_the_old_tool_opening_without_an_error_and_reopens_the_retained_conversation()
 {
    let world = World::new().await;
    let thread = world
        .thread("A retained conversation gets a fresh physical opening")
        .await;
    let turn = start(&world, thread, world.ada).await;
    world
        .say(
            thread,
            crate::common::turn_ended(turn, orch_link_contract::events::TurnOutcome::Completed),
        )
        .await;
    world.wait_state(thread, "idle").await;
    let opening = world.opening_of(thread).await;
    let segment = world.segment_of(thread).await;
    let native = world
        .scalar::<String>(
            "SELECT native_session_ref FROM segments WHERE id=$1",
            segment,
        )
        .await;
    world
        .say_session_taken_in(
            thread,
            Event::SessionLost(SessionLost {
                uncertain: false,
                opening_id: opening,
                expected: true,
                reason: "the connector restarted while the conversation was idle".into(),
            }),
        )
        .await;
    assert_eq!(world.state(thread).await.0, "idle");
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM items WHERE thread_id=$1 AND kind='error'",
                thread
            )
            .await,
        0
    );
    let facts: Json = world
        .scalar(
            "SELECT delivered_resources->'tool_admission' FROM segments WHERE id=$1",
            segment,
        )
        .await;
    assert_eq!(facts["live"], json!(false));
    assert_eq!(
        world
            .scalar::<String>(
                "SELECT native_session_ref FROM segments WHERE id=$1",
                segment
            )
            .await,
        native
    );
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Next message"))
        .await
        .unwrap();
    let _ = world.turn_start(thread, 1).await;
    assert_ne!(
        world.opening_of(thread).await,
        opening,
        "resuming the retained conversation takes a newly admitted opening"
    );
    assert_eq!(
        world
            .scalar::<String>(
                "SELECT native_session_ref FROM segments WHERE id=$1",
                segment
            )
            .await,
        native
    );
}

async fn start(world: &World, thread: Id, author: Id) -> Id {
    world
        .engine()
        .submit_input(
            &Caller::session(author),
            thread,
            queued("Change the file once"),
        )
        .await
        .unwrap();
    let turn = world.turn_start(thread, 0).await;
    let segment = world.segment_of(thread).await;
    let recorded =
        orch_db::queries::threads::segment(&mut world.pool.acquire().await.unwrap(), segment)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        recorded.machine_id,
        Some(world.machine_id),
        "the actual session open records its recipient before any fixture report can assign it"
    );
    world.say(thread, turn_began(turn.turn_id)).await;
    world.wait_state(thread, "running").await;
    turn.turn_id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_session_on_its_recorded_machine_continues_without_a_fixture_assignment() {
    let world = World::new().await;
    let thread = world.thread("The native open owns its source").await;
    let segment = world.segment_of(thread).await;
    let unopened =
        orch_db::queries::threads::segment(&mut world.pool.acquire().await.unwrap(), segment)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        unopened.machine_id, None,
        "a provisional segment has no native session yet"
    );
    let first = start(&world, thread, world.ada).await;
    lost(&world, thread, first).await;
    let next = world.turn_start(thread, 1).await;
    assert_eq!(next.text, "Continue");
    assert_eq!(
        recovery(&world, thread).await.context["native_machine"],
        json!(world.machine_id)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_session_without_a_recorded_machine_does_not_borrow_the_environments_current_machine()
 {
    let world = World::new().await;
    let thread = world
        .thread("An unknown native source needs a person")
        .await;
    let first = start(&world, thread, world.ada).await;
    let segment = world.segment_of(thread).await;
    let mut gate = world.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM segments WHERE id=$1 FOR UPDATE")
        .bind(segment)
        .fetch_one(&mut *gate)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *gate)
        .await
        .unwrap();
    world
        .say(
            thread,
            Event::SessionLost(SessionLost {
                expected: false,
                opening_id: world.opening_of(thread).await,
                uncertain: false,
                reason: "The original process ended with no surviving source fact".into(),
            }),
        )
        .await;
    tokio::time::timeout(crate::common::PATIENCE, async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%SELECT delivered_resources#>>%' AND query LIKE '%FOR SHARE%')")
                .bind(pid).fetch_one(world.pool.pool()).await.unwrap();
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the accepted loss waits at its physical source read after sender authorization");
    // Remove the source only after the report passed sender authorization and before its physical
    // facts are read. No fixture report or broader actor fills it from the environment.
    sqlx::query("UPDATE segments SET machine_id=NULL WHERE id=$1")
        .bind(segment)
        .execute(&mut *gate)
        .await
        .unwrap();
    gate.commit().await.unwrap();
    world.wait_turn(first, "failed").await;
    world.engine().keep_call_recoveries().await.unwrap();
    let recorded =
        orch_db::queries::threads::segment(&mut world.pool.acquire().await.unwrap(), segment)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(recorded.machine_id, None);
    assert_eq!(
        world
            .scalar::<Option<Id>>(
                "SELECT machine_id FROM environments WHERE id=$1",
                world.environment
            )
            .await,
        Some(world.machine_id)
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM model_call_recovery WHERE node_id=$1 AND purpose='agent'",
                thread
            )
            .await,
        0
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM model_call_recovery_attempts WHERE node_id=$1",
                thread
            )
            .await,
        0
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
                thread
            )
            .await,
        1
    );
    assert_eq!(world.turn_starts(thread).len(), 1);
}

async fn lost(world: &World, thread: Id, turn: Id) {
    world
        .say(
            thread,
            Event::SessionLost(SessionLost {
                expected: false,
                opening_id: world.opening_of(thread).await,
                uncertain: false,
                reason: "The scripted native process ended".to_owned(),
            }),
        )
        .await;
    world.wait_turn(turn, "failed").await;
}

async fn parked(world: &World, thread: Id) {
    eventually("native recovery is waiting for its machine", || async {
        let row = recovery(world, thread).await;
        row.state == "scheduled"
            && row.due_at.is_none()
            && row.context["wait_machine"] == json!(world.machine_id)
    })
    .await;
}

async fn control(world: &World, thread: Id, action: CallRecoveryAction) {
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_action(&mut tx)
        .await
        .unwrap();
    let principal = orch_authz::Principal::load(
        &mut tx,
        Caller::session(world.ada),
        orch_types::Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    world
        .engine()
        .control_call_recovery(tx, principal, thread, action)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_confirmed_lost_session_continues_once_without_replaying_an_unreported_effect() {
    let world = World::new().await;
    let author = world.person("work_author").await;
    world.give(author, "developer").await;
    let thread = world.thread_by(author, "Retain native work").await;
    let first = start(&world, thread, author).await;
    assert_eq!(world.count("SELECT count(*) FROM items WHERE turn_id=$1 AND kind IN ('agent_message','tool_call')", first).await, 0);
    lost(&world, thread, first).await;
    let next = world.turn_start(thread, 1).await;
    world.say(thread, turn_began(next.turn_id)).await;
    world.wait_state(thread, "running").await;
    let mut conn = world.pool.acquire().await.unwrap();
    let original_id: Id = sqlx::query_scalar("SELECT input_id FROM turns WHERE id=$1")
        .bind(first)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let next_id: Id = sqlx::query_scalar("SELECT input_id FROM turns WHERE id=$1")
        .bind(next.turn_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let original = orch_db::queries::threads::input(&mut conn, original_id)
        .await
        .unwrap()
        .unwrap();
    let derived = orch_db::queries::threads::input(&mut conn, next_id)
        .await
        .unwrap()
        .unwrap();
    drop(conn);
    assert_ne!(derived.id, original.id);
    assert_eq!(derived.author_id, Some(author));
    assert_ne!(derived.author_id, Some(world.ada));
    assert_eq!(derived.submission_authority, original.submission_authority);
    assert_eq!(derived.run_as, original.run_as);
    assert_eq!(derived.run_as_authority, original.run_as_authority);
    assert_eq!(derived.continuation_id, original.continuation_id);
    assert_eq!(derived.content, json!([{"type":"text","text":"Continue"}]));
    let row = recovery(&world, thread).await;
    assert_eq!(row.rule["id"], "session-lost");
    assert_eq!(row.context["effects"], true);
    let marker: Json = world.scalar("SELECT payload FROM items WHERE thread_id=$1 AND kind='marker' AND payload->>'marker'='call_recovery' ORDER BY seq_created DESC LIMIT 1", thread).await;
    assert_eq!(marker["data"]["kind"], "session_lost");
    assert_eq!(marker["data"]["action"], "continue");
    assert_eq!(marker["data"]["attempt"], 1);
    assert_eq!(marker["data"]["total"], 1);
    lost(&world, thread, next.turn_id).await;
    assert_eq!(recovery(&world, thread).await.state, "waiting");
    world.engine().keep_call_recoveries().await.unwrap();
    assert_eq!(world.turn_starts(thread).len(), 2);
    assert_eq!(world.count("SELECT count(*) FROM model_call_recovery_attempts WHERE node_id=$1 AND failure_kind='session_lost' AND automatic", thread).await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_offline_machine_holds_the_reserved_continuation_until_its_return_signal() {
    let world = World::new().await;
    let thread = world.thread("Wait for the native machine").await;
    let first = start(&world, thread, world.ada).await;
    world.set_machine_online(world.machine_id, false).await;
    lost(&world, thread, first).await;
    parked(&world, thread).await;
    for _ in 0..3 {
        world.engine().keep_call_recoveries().await.unwrap();
    }
    assert_eq!(world.turn_starts(thread).len(), 1);
    assert_eq!(world.count("SELECT count(*) FROM model_call_recovery_attempts WHERE node_id=$1 AND failure_kind='session_lost' AND automatic", thread).await, 1);
    let view = orch_engine::calls::view(&mut world.pool.acquire().await.unwrap(), thread)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(view.detail.as_deref(), Some("machine_offline"));
    assert!(view.due_at.is_none());
    let displays = orch_engine::views::thread_displays(
        &mut world.pool.acquire().await.unwrap(),
        &[thread],
        Some(world.ada),
        orch_types::Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    let display = &displays[&thread];
    let status = display.recovery.as_ref().unwrap();
    assert_eq!(
        display.state,
        orch_api_contract::threads::ThreadDisplayState::Queued
    );
    assert_eq!(
        status.failure_kind,
        orch_api_contract::threads::ThreadRecoveryFailure::SessionLost
    );
    assert_eq!(
        status.stop_reason,
        Some(orch_api_contract::threads::ThreadRecoveryStop::MachineOffline)
    );
    world.set_machine_online(world.machine_id, true).await;
    world
        .engine()
        .machine_online(world.machine_id)
        .await
        .unwrap();
    let next = world.turn_start(thread, 1).await;
    world
        .engine()
        .machine_online(world.machine_id)
        .await
        .unwrap();
    world.engine().keep_call_recoveries().await.unwrap();
    assert_eq!(world.turn_starts(thread).len(), 2);
    let content: Json = world
        .scalar(
            "SELECT i.content FROM turns t JOIN thread_inputs i ON i.id=t.input_id WHERE t.id=$1",
            next.turn_id,
        )
        .await;
    assert_eq!(content, json!([{"type":"text","text":"Continue"}]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stopping_a_parked_session_recovery_prevents_a_machine_return_from_releasing_it() {
    let world = World::new().await;
    let thread = world.thread("Stop native recovery").await;
    let first = start(&world, thread, world.ada).await;
    world.set_machine_online(world.machine_id, false).await;
    lost(&world, thread, first).await;
    parked(&world, thread).await;
    control(&world, thread, CallRecoveryAction::Stop).await;
    world.set_machine_online(world.machine_id, true).await;
    world
        .engine()
        .machine_online(world.machine_id)
        .await
        .unwrap();
    world.engine().keep_call_recoveries().await.unwrap();
    assert_eq!(recovery(&world, thread).await.state, "stopped");
    assert_eq!(world.turn_starts(thread).len(), 1);
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
                thread
            )
            .await,
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_returned_machine_cannot_replace_the_original_authors_revoked_authority() {
    let world = World::new().await;
    let author = world.person("revocable_author").await;
    let grant = world.give(author, "developer").await;
    let thread = world.thread("Keep the original authority").await;
    let first = start(&world, thread, author).await;
    world.set_machine_online(world.machine_id, false).await;
    lost(&world, thread, first).await;
    parked(&world, thread).await;
    world.take(grant).await;
    world.set_machine_online(world.machine_id, true).await;
    world
        .engine()
        .machine_online(world.machine_id)
        .await
        .unwrap();
    world.engine().keep_call_recoveries().await.unwrap();
    let row = recovery(&world, thread).await;
    assert_eq!(row.state, "stopped");
    assert!(matches!(
        row.context["detail"].as_str(),
        Some("origin_authority" | "author_lacks_interact")
    ));
    assert_eq!(world.turn_starts(thread).len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_changed_rule_is_rechecked_when_the_lost_sessions_machine_returns() {
    let world = World::new().await;
    let thread = world.thread("Recovery policy can change").await;
    let first = start(&world, thread, world.ada).await;
    world.set_machine_online(world.machine_id, false).await;
    lost(&world, thread, first).await;
    parked(&world, thread).await;
    world.set(world.topic, "calls.rules", json!([])).await;
    world.set_machine_online(world.machine_id, true).await;
    world
        .engine()
        .machine_online(world.machine_id)
        .await
        .unwrap();
    world.engine().keep_call_recoveries().await.unwrap();
    let row = recovery(&world, thread).await;
    assert_eq!(row.state, "stopped");
    assert_eq!(row.context["detail"], "rules_changed");
    assert_eq!(world.turn_starts(thread).len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inherited_and_locked_session_loss_rules_use_the_existing_repeat_guard() {
    let world = World::new().await;
    let thread = world.thread("Bound native recoveries").await;
    world.set(world.project, "calls.rules", json!([{"id":"recover-native","when":{"kind":"session_lost","purpose":"agent"},"steps":[{"type":"continue","times":20}]}])).await;
    world.set(world.topic, "calls.rules", json!([])).await;
    sqlx::query("UPDATE settings SET locked=true WHERE node_id=$1 AND key='calls.rules'")
        .bind(world.project)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.set(world.topic, "calls.repeat_limit", json!(2)).await;
    let mut turn = start(&world, thread, world.ada).await;
    for index in 0..3 {
        lost(&world, thread, turn).await;
        if index < 2 {
            let next = world.turn_start(thread, index + 1).await;
            world.say(thread, turn_began(next.turn_id)).await;
            world.wait_state(thread, "running").await;
            turn = next.turn_id;
        }
    }
    let row = recovery(&world, thread).await;
    assert_eq!(row.state, "stopped");
    assert_eq!(row.rule["id"], "recover-native");
    assert_eq!(row.action.unwrap()["detail"], "repeat_limit");
    assert_eq!(world.turn_starts(thread).len(), 3);
    assert_eq!(world.count("SELECT count(*) FROM model_call_recovery_attempts WHERE node_id=$1 AND failure_kind='session_lost' AND automatic", thread).await, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_recovery_off_includes_confirmed_native_session_loss() {
    let world = World::new().await;
    let thread = world.thread("Native recovery is configurable").await;
    world.set(world.topic, "calls.repeat_limit", json!(0)).await;
    let first = start(&world, thread, world.ada).await;
    lost(&world, thread, first).await;
    world.engine().keep_call_recoveries().await.unwrap();
    let row = recovery(&world, thread).await;
    assert_eq!(row.state, "stopped");
    assert_eq!(row.action.unwrap()["detail"], "repeat_limit");
    assert_eq!(world.turn_starts(thread).len(), 1);
    assert_eq!(world.count("SELECT count(*) FROM model_call_recovery_attempts WHERE node_id=$1 AND failure_kind='session_lost' AND automatic", thread).await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_native_recovery_survives_a_controller_restart_and_a_new_presence_signal() {
    let world = World::new().await;
    let thread = world.thread("Durable native recovery").await;
    let first = start(&world, thread, world.ada).await;
    world.set_machine_online(world.machine_id, false).await;
    lost(&world, thread, first).await;
    parked(&world, thread).await;
    world.restart_api().await;
    let next = world.turn_start(thread, 1).await;
    assert_ne!(next.turn_id, first);
    assert_eq!(world.count("SELECT count(*) FROM model_call_recovery_attempts WHERE node_id=$1 AND failure_kind='session_lost' AND automatic", thread).await, 1);
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
                thread
            )
            .await,
        2
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unconfirmed_start_or_a_persons_stop_never_blindly_replays_a_lost_prompt() {
    for confirmed in [false, true] {
        let world = World::new().await;
        let thread = world.thread("A person decides uncertain work").await;
        world
            .engine()
            .submit_input(
                &Caller::session(world.ada),
                thread,
                queued("A single change"),
            )
            .await
            .unwrap();
        let first = world.turn_start(thread, 0).await;
        if confirmed {
            world.say(thread, turn_began(first.turn_id)).await;
            world.wait_state(thread, "running").await;
            world
                .engine()
                .interrupt(&Caller::session(world.ada), thread, first.turn_id)
                .await
                .unwrap();
        }
        lost(&world, thread, first.turn_id).await;
        world.engine().keep_call_recoveries().await.unwrap();
        assert_eq!(world.turn_starts(thread).len(), 1);
        assert_eq!(
            world
                .count(
                    "SELECT count(*) FROM model_call_recovery WHERE node_id=$1 AND purpose='agent'",
                    thread
                )
                .await,
            0
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn physical_loss_after_a_queued_close_finishes_the_delivered_facts_without_recovering_the_withdrawn_opening()
 {
    use orch_db::queries::link::{NewCommand, insert_command};
    let world = World::new().await;
    let thread = world
        .thread("The closed admission still receives its actual end")
        .await;
    let turn = start(&world, thread, world.ada).await;
    world
        .say(
            thread,
            crate::common::turn_ended(turn, orch_link_contract::events::TurnOutcome::Completed),
        )
        .await;
    world.wait_state(thread, "idle").await;
    let segment = world.segment_of(thread).await;
    let opening = world.opening_of(thread).await;
    let body = json!({"thread_id":thread,"segment_id":segment});
    insert_command(
        &mut world.pool.acquire().await.unwrap(),
        &NewCommand {
            native_target: None,
            id: orch_types::new_id(),
            machine_id: world.machine_id,
            kind: "session.close.fenced",
            body: &body,
            serial_key: None,
            deadline_at: world.clock.now() + chrono::Duration::minutes(2),
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    world
        .say_session_taken_in(
            thread,
            Event::SessionLost(SessionLost {
                uncertain: false,
                opening_id: opening,
                expected: false,
                reason: "the already withdrawn physical opening ended".into(),
            }),
        )
        .await;
    let facts: Json = world
        .scalar(
            "SELECT delivered_resources FROM segments WHERE id=$1",
            segment,
        )
        .await;
    assert_eq!(
        facts["state"],
        json!("closed"),
        "an admission withdrawal does not discard the later factual close"
    );
    assert_eq!(facts["tool_admission"]["live"], json!(false));
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM items WHERE thread_id=$1 AND kind='error'",
                thread
            )
            .await,
        0
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM model_call_recovery WHERE node_id=$1",
                thread
            )
            .await,
        0
    );
}

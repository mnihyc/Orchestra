//! Background completion preferences cross the ordinary inherited session boundary.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]
mod common;
use common::{World, queued, steering, turn_ended};
use orch_authz::Caller;
use orch_link_contract::commands::{Command, CommandKind};
use orch_link_contract::events::TurnOutcome;
use serde_json::json;

fn session_open_count(world: &World, transport_only: bool) -> usize {
    world
        .seen
        .all()
        .iter()
        .filter(|command| match command {
            Command::SessionOpen(open) => open.transport_only == transport_only,
            Command::SessionOpenFenced(open) => open.session.transport_only == transport_only,
            _ => false,
        })
        .count()
}

async fn task_recorded(
    world: &World,
    thread: orch_types::Id,
    task: &orch_link_contract::background::BackgroundTask,
    state: &str,
) {
    common::eventually(
        "the actual task lifecycle report to be committed",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM items i
              CROSS JOIN LATERAL jsonb_each(i.payload->'background_tasks') entry
              WHERE i.thread_id=$1 AND i.native_ref=$2
                AND entry.value->'task'->>'id'=$3
                AND entry.value->'task'->>'run_id' IS NOT DISTINCT FROM $4::text
                AND entry.value->'task'->>'state'=$5)",
            )
            .bind(thread)
            .bind(&task.native_item_ref)
            .bind(&task.id)
            .bind(&task.run_id)
            .bind(state)
            .fetch_one(world.pool.pool())
            .await
            .unwrap()
        },
    )
    .await;
}

async fn command_task(
    world: &World,
    thread: orch_types::Id,
    turn: orch_types::Id,
) -> orch_link_contract::background::BackgroundTask {
    use orch_link_contract::background::{
        BackgroundTask, BackgroundTaskKind, BackgroundTaskState, BackgroundTasks,
    };
    let started = orch_link_contract::events::Event::decode("item.started", json!({
        "turn_id":turn,"native_ref":"background-command","stream":"main","parent_native_ref":null,
        "kind":"tool_call","payload":{
            "tool":{"kind":"command","command":"sleep 1","cwd":"/workspace","exit_code":null,
                "background":true,"process_id":"task","actions":[],"output":{"inline":"","total_len":0,"truncated":false,"blob":null}},
            "title":"Build","started_at":world.clock.time(),"duration_ms":null,
            "raw":{"name":"Bash","input":{"inline":"sleep 1","total_len":7,"truncated":false,"blob":null},"output":null,"is_error":false}
        }
    })).unwrap();
    world.say(thread, started).await;
    let task = BackgroundTask {
        id: "task".into(),
        run_id: Some("run".into()),
        native_item_ref: "background-command".into(),
        origin_turn: turn,
        kind: BackgroundTaskKind::Command,
        label: "Build".into(),
        state: BackgroundTaskState::Running,
        exit_code: None,
        can_end: true,
        name: None,
        nickname: None,
        role: None,
    };
    world
        .say(
            thread,
            orch_link_contract::events::Event::BackgroundTasks(BackgroundTasks {
                self_waking: true,
                native_idle: false,
                tasks: vec![task.clone()],
                native_wake: None,
            }),
        )
        .await;
    task_recorded(world, thread, &task, "running").await;
    task
}

async fn native_finished(
    world: &World,
    thread: orch_types::Id,
    mut task: orch_link_contract::background::BackgroundTask,
) {
    use orch_link_contract::background::{
        BackgroundTaskState, BackgroundTasks, NativeBackgroundWake,
    };
    task.state = BackgroundTaskState::Completed;
    task.exit_code = Some(0);
    let key = json!([task.id, task.run_id]).to_string();
    world
        .say(
            thread,
            orch_link_contract::events::Event::BackgroundTasks(BackgroundTasks {
                self_waking: true,
                native_idle: false,
                tasks: vec![task],
                native_wake: Some(NativeBackgroundWake {
                    token: "native-followup".into(),
                    task_ids: vec![key],
                }),
            }),
        )
        .await;
}

#[tokio::test]
async fn background_wake_is_on_by_default_and_an_inherited_change_waits_for_session_open() {
    let world = World::new().await;
    let thread = world.thread("Background completion").await;
    let (running, _) = world.running_turn(thread, "Begin").await;
    let opens = world.seen.of_kind(CommandKind::SessionOpen);
    assert!(
        opens
            .iter()
            .any(|command| matches!(command, Command::SessionOpen(open) if open.background_wake))
    );
    world
        .set(world.topic, "threads.background_wake", json!("off"))
        .await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            steering("Keep working"),
        )
        .await
        .unwrap();
    world.wait_ran(CommandKind::TurnSteer, 1).await;
    assert_eq!(
        world.seen.of_kind(CommandKind::SessionOpen).len(),
        opens.len()
    );
    world
        .say(thread, turn_ended(running.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go on"))
        .await
        .unwrap();
    world.turn_start(thread, 1).await;
    let opens = world.seen.of_kind(CommandKind::SessionOpen);
    let last = opens
        .iter()
        .rev()
        .find_map(|command| match command {
            Command::SessionOpen(open) => Some(open),
            _ => None,
        })
        .unwrap();
    assert!(!last.background_wake);
}

#[tokio::test]
async fn a_native_followup_uses_the_original_author_and_records_its_cause_before_output() {
    let world = World::new().await;
    let thread = world.thread("Native completion").await;
    let (first, submitted) = world.running_turn(thread, "Begin").await;
    let task = command_task(&world, thread, first.turn_id).await;
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    native_finished(&world, thread, task).await;
    let next = world.turn_start(thread, 1).await;
    assert_eq!(
        world.seen.of_kind(CommandKind::TurnInterrupt).len(),
        0,
        "preparation keeps the native continuation parked until admission"
    );
    assert_eq!(next.native_wake.as_deref(), Some("native-followup"));
    let (author, origin, marker_turn): (Option<orch_types::Id>, String, Option<orch_types::Id>) =
        sqlx::query_as("SELECT q.author_id,i.payload->'data'->>'origin_input',i.turn_id FROM items i JOIN thread_inputs q ON q.id=(i.payload->'data'->>'input_id')::uuid WHERE i.thread_id=$1 AND i.payload->>'marker'='background_wake'")
        .bind(thread).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(author, Some(world.ada));
    assert_eq!(origin, submitted.input.id.to_string());
    assert_eq!(marker_turn, Some(next.turn_id));
    assert_ne!(first.turn_id, next.turn_id);
}

#[tokio::test]
async fn a_refused_native_followup_is_interrupted_and_keeps_its_completion_pending() {
    let world = World::new().await;
    let thread = world.thread("Held native completion").await;
    let (first, _) = world.running_turn(thread, "Begin").await;
    let task = command_task(&world, thread, first.turn_id).await;
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    world.access.deny(world.ada);
    native_finished(&world, thread, task).await;
    world.wait_ran(CommandKind::TurnInterrupt, 1).await;
    let (state, detail): (String, Option<String>) = sqlx::query_as(
        "SELECT state,state_detail FROM thread_inputs WHERE thread_id=$1 AND source='system' ORDER BY created_at DESC LIMIT 1")
        .bind(thread).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(state, "pending");
    assert_eq!(detail.as_deref(), Some("model_use_required"));
    assert_eq!(world.turn_starts(thread).len(), 1);
}

#[tokio::test]
async fn losing_interact_holds_a_native_completion_instead_of_cancelling_its_original_work() {
    let world = World::new().await;
    let thread = world.thread("Shared native completion").await;
    let author = world.person("worker").await;
    let role = world.give(author, "interactor").await;
    world
        .engine()
        .submit_input(&Caller::session(author), thread, queued("Begin"))
        .await
        .unwrap();
    let first = world.turn_start(thread, 0).await;
    world.say(thread, common::turn_began(first.turn_id)).await;
    world.wait_turn(first.turn_id, "running").await;
    let task = command_task(&world, thread, first.turn_id).await;
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    world.take(role).await;
    native_finished(&world, thread, task).await;
    world.wait_ran(CommandKind::TurnInterrupt, 1).await;
    let (state, detail): (String, Option<String>) = sqlx::query_as(
        "SELECT state,state_detail FROM thread_inputs WHERE thread_id=$1 AND source='system' ORDER BY created_at DESC LIMIT 1")
        .bind(thread).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(state, "pending");
    assert_eq!(detail.as_deref(), Some("author_lost_access"));
    assert_eq!(world.turn_starts(thread).len(), 1);
}

#[tokio::test]
async fn a_late_terminal_failure_starts_one_controller_turn_with_the_exact_completion_words() {
    use orch_link_contract::background::{BackgroundTaskState, BackgroundTasks};
    let world = World::new().await;
    let thread = world.thread("Controller completion").await;
    let (first, _) = world.running_turn(thread, "Begin").await;
    let mut task = command_task(&world, thread, first.turn_id).await;
    let item: orch_types::Id = sqlx::query_scalar(
        "SELECT id FROM items WHERE thread_id=$1 AND native_ref='background-command'",
    )
    .bind(thread)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    task.state = BackgroundTaskState::Failed;
    task.exit_code = Some(2);
    let report = BackgroundTasks {
        self_waking: false,
        native_idle: false,
        tasks: vec![task],
        native_wake: None,
    };
    world
        .say(
            thread,
            orch_link_contract::events::Event::BackgroundTasks(report.clone()),
        )
        .await;
    let next = world.turn_start(thread, 1).await;
    assert_eq!(next.native_wake, None);
    assert_eq!(
        next.text,
        format!("Background task \"Build\" finished (exit 2). Its output is in the item {item}.")
    );
    world
        .say(
            thread,
            orch_link_contract::events::Event::BackgroundTasks(report),
        )
        .await;
    let inputs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM thread_inputs WHERE thread_id=$1 AND source='system'",
    )
    .bind(thread)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        inputs, 1,
        "replayed completion cannot reserve a second continuation"
    );
}

#[tokio::test]
async fn a_late_child_completion_starts_a_controller_turn_with_its_reported_name() {
    use orch_link_contract::background::{
        BackgroundTask, BackgroundTaskKind, BackgroundTaskState, BackgroundTasks,
    };
    let world = World::new().await;
    let thread = world.thread("Child completion").await;
    let (first, _) = world.running_turn(thread, "Begin").await;
    world.say(thread, orch_link_contract::events::Event::decode("item.started", json!({
        "turn_id":first.turn_id,"native_ref":"spawn","stream":"main","parent_native_ref":null,
        "kind":"tool_call","payload":{
            "tool":{"kind":"subagent","child_stream":"sub:child","name":"Review changes","role":"reviewer","prompt":"Review","state":"running","model":null},
            "title":"Review changes","started_at":world.clock.time(),"duration_ms":null,
            "raw":{"name":"spawnAgent","input":{"inline":"","total_len":0,"truncated":false,"blob":null},"output":null,"is_error":false}
        }
    })).unwrap()).await;
    let mut task = BackgroundTask {
        id: "child".into(),
        run_id: None,
        native_item_ref: "spawn".into(),
        origin_turn: first.turn_id,
        kind: BackgroundTaskKind::Subagent,
        label: "Review changes".into(),
        name: Some("Review changes".into()),
        nickname: Some("Finch".into()),
        role: Some("reviewer".into()),
        state: BackgroundTaskState::Running,
        exit_code: None,
        can_end: false,
    };
    world
        .say(
            thread,
            orch_link_contract::events::Event::BackgroundTasks(BackgroundTasks {
                self_waking: false,
                native_idle: false,
                tasks: vec![task.clone()],
                native_wake: None,
            }),
        )
        .await;
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    task.state = BackgroundTaskState::Completed;
    world
        .say(
            thread,
            orch_link_contract::events::Event::BackgroundTasks(BackgroundTasks {
                self_waking: false,
                native_idle: false,
                tasks: vec![task],
                native_wake: None,
            }),
        )
        .await;
    let next = world.turn_start(thread, 1).await;
    assert_eq!(next.text, "Subagent \"Review changes\" finished.");
    assert_eq!(next.native_wake, None);
}

#[tokio::test]
async fn running_work_retains_one_lease_for_both_authors_until_the_last_task_ends() {
    use orch_link_contract::background::{BackgroundTaskState, BackgroundTasks};
    let world = World::on_a_key("scripted-background-provider").await;
    let thread = world.thread("Shared running work").await;
    let (first, _) = world.running_turn(thread, "Begin").await;
    let mut task = command_task(&world, thread, first.turn_id).await;
    let lease = world.open_lease_of(thread).await;
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    assert_eq!(world.open_lease_of(thread).await, lease);
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Same author"))
        .await
        .unwrap();
    let second = world.turn_start(thread, 1).await;
    assert_eq!(
        world.open_lease_of(thread).await,
        lease,
        "live child requests keep their original authority"
    );
    world
        .say(thread, turn_ended(second.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    let author = world.person("reviewer").await;
    world.give(author, "interactor").await;
    let submitted = world
        .engine()
        .submit_input(&Caller::session(author), thread, queued("Other author"))
        .await
        .unwrap();
    let third = world.turn_start(thread, 2).await;
    assert_ne!(
        submitted.input.state_detail.as_deref(),
        Some("background_author")
    );
    assert_eq!(world.open_lease_of(thread).await, lease);
    assert_eq!(session_open_count(&world, false), 1);
    assert_eq!(
        session_open_count(&world, true),
        2,
        "each later forwarded turn rebinds transport without reopening the native session"
    );
    task.state = BackgroundTaskState::Stopped;
    world
        .say(
            thread,
            orch_link_contract::events::Event::BackgroundTasks(BackgroundTasks {
                self_waking: true,
                native_idle: false,
                tasks: vec![task],
                native_wake: None,
            }),
        )
        .await;
    world
        .say(thread, turn_ended(third.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    world
        .engine()
        .submit_input(
            &Caller::session(author),
            thread,
            queued("After all work ends"),
        )
        .await
        .unwrap();
    let fourth = world.turn_start(thread, 3).await;
    assert_ne!(world.open_lease_of(thread).await, lease);
    assert!(
        !world
            .scalar::<bool>(
                "SELECT background_overlap FROM turns WHERE id=$1",
                fourth.turn_id
            )
            .await
    );
}

#[tokio::test]
async fn instruction_refresh_waits_for_live_work_without_replacing_its_session_generation() {
    use orch_link_contract::background::{BackgroundTaskState, BackgroundTasks};
    let world = World::new().await;
    let thread = world.thread("Deferred instructions").await;
    let (first, _) = world.running_turn(thread, "Begin").await;
    let mut task = command_task(&world, thread, first.turn_id).await;
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    let segment = world.segment_of(thread).await;
    world
        .set(
            world.topic,
            "agent.instructions",
            json!("NEW-INSTRUCTIONS-AFTER-TASKS"),
        )
        .await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("While work runs"),
        )
        .await
        .unwrap();
    let second = world.turn_start(thread, 1).await;
    assert_eq!(world.segment_of(thread).await, segment);
    assert_eq!(
        world
            .seen
            .of_kind(CommandKind::SessionOpen)
            .iter()
            .filter(|command| matches!(command, Command::SessionOpen(open) if !open.transport_only))
            .count(),
        1,
        "instruction refresh cannot restart a live generation"
    );
    world
        .say(thread, turn_ended(second.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    task.state = BackgroundTaskState::Stopped;
    world
        .say(
            thread,
            orch_link_contract::events::Event::BackgroundTasks(BackgroundTasks {
                self_waking: true,
                native_idle: false,
                tasks: vec![task],
                native_wake: None,
            }),
        )
        .await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("After work ends"),
        )
        .await
        .unwrap();
    world.turn_start(thread, 2).await;
    assert!(world.seen.of_kind(CommandKind::SessionOpen).iter().any(|command| {
        matches!(command, Command::SessionOpen(open) if open.instructions.contains("NEW-INSTRUCTIONS-AFTER-TASKS"))
    }));
}

#[tokio::test]
async fn human_input_without_a_background_hold_preserves_the_existing_ordinary_queue_hold() {
    let world = World::new().await;
    let thread = world.thread("Ordinary queue hold").await;
    sqlx::query("UPDATE threads SET queue_held=true WHERE node_id=$1")
        .bind(thread)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let sent = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Keep this queued"),
        )
        .await
        .unwrap();
    assert!(sent.input.queue_held);
    world.engine().preflight(thread).await.unwrap();
    let held: bool = sqlx::query_scalar("SELECT queue_held FROM threads WHERE node_id=$1")
        .bind(thread)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert!(held);
    assert!(world.turn_starts(thread).is_empty());
}

#[tokio::test]
async fn stop_holds_its_future_completion_until_human_input_without_resuming_the_ordinary_queue() {
    use orch_link_contract::background::{BackgroundTaskState, BackgroundTasks};
    let world = World::new().await;
    let thread = world.thread("Stop completion hold").await;
    let (first, _) = world.running_turn(thread, "Begin").await;
    let mut task = command_task(&world, thread, first.turn_id).await;
    let caller = Caller::session(world.ada);
    let waiting = world
        .engine()
        .submit_input(&caller, thread, queued("Already queued"))
        .await
        .unwrap();
    world
        .engine()
        .interrupt(&caller, thread, first.turn_id)
        .await
        .unwrap();
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Interrupted))
        .await;
    world.wait_turn(first.turn_id, "interrupted").await;
    task.state = BackgroundTaskState::Completed;
    task.exit_code = Some(0);
    world
        .say(
            thread,
            orch_link_contract::events::Event::BackgroundTasks(BackgroundTasks {
                self_waking: false,
                native_idle: false,
                tasks: vec![task],
                native_wake: None,
            }),
        )
        .await;
    common::eventually("the stopped turn's completion to be held", || async {
        sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM thread_inputs WHERE thread_id=$1 AND state_detail='background_stopped')")
            .bind(thread).fetch_one(world.pool.pool()).await.unwrap()
    }).await;
    assert_eq!(world.turn_starts(thread).len(), 1);
    world
        .engine()
        .submit_input(&caller, thread, queued("I am back"))
        .await
        .unwrap();
    let next = world.turn_start(thread, 1).await;
    assert!(next.text.starts_with("Background task \"Build\" finished"));
    let held: bool = sqlx::query_scalar("SELECT queue_held FROM thread_inputs WHERE id=$1")
        .bind(waiting.input.id)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert!(
        held,
        "a new input releases completion holds but does not resume the ordinary queue"
    );
}

#[tokio::test]
async fn stopping_a_later_turn_preserves_an_earlier_tasks_wake() {
    use orch_link_contract::background::{BackgroundTaskState, BackgroundTasks};
    let world = World::new().await;
    let thread = world.thread("Earlier task survives").await;
    let (first, _) = world.running_turn(thread, "Begin").await;
    let mut task = command_task(&world, thread, first.turn_id).await;
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Later turn"))
        .await
        .unwrap();
    let second = world.turn_start(thread, 1).await;
    world.say(thread, common::turn_began(second.turn_id)).await;
    world.wait_turn(second.turn_id, "running").await;
    world
        .engine()
        .interrupt(&Caller::session(world.ada), thread, second.turn_id)
        .await
        .unwrap();
    world
        .say(thread, turn_ended(second.turn_id, TurnOutcome::Interrupted))
        .await;
    world.wait_turn(second.turn_id, "interrupted").await;
    task.state = BackgroundTaskState::Completed;
    world
        .say(
            thread,
            orch_link_contract::events::Event::BackgroundTasks(BackgroundTasks {
                self_waking: false,
                native_idle: false,
                tasks: vec![task],
                native_wake: None,
            }),
        )
        .await;
    let next = world.turn_start(thread, 2).await;
    assert!(next.text.starts_with("Background task \"Build\" finished"));
}

#[tokio::test]
async fn a_lost_session_records_the_loss_and_never_revives_a_task_from_late_history() {
    use orch_link_contract::background::BackgroundTasks;
    let world = World::new().await;
    let thread = world.thread("Lost background work").await;
    let (first, _) = world.running_turn(thread, "Begin").await;
    let task = command_task(&world, thread, first.turn_id).await;
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    world
        .say(
            thread,
            orch_link_contract::events::Event::SessionLost(
                orch_link_contract::events::SessionLost {
                    reason: "the native process ended".into(),
                },
            ),
        )
        .await;
    task_recorded(&world, thread, &task, "lost").await;
    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM items WHERE thread_id=$1 AND native_ref='background-command'",
    )
    .bind(thread)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    let facts = payload["background_tasks"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert_eq!(facts["task"]["state"], "lost");
    assert!(facts.get("lost_at").is_some());
    let stream = orch_link_contract::streams::Stream::Session {
        thread_id: thread,
        segment_id: world.segment_of(thread).await,
    };
    // The loss row and its stream cursor committed together. Waiting past that
    // exact cursor proves the late report was handled, even when it is discarded.
    let through_loss =
        orch_db::queries::link::cursors(&mut world.pool.acquire().await.unwrap(), world.machine_id)
            .await
            .unwrap()
            .get(&stream.to_string())
            .copied()
            .unwrap_or(0);
    world
        .say(
            thread,
            orch_link_contract::events::Event::BackgroundTasks(BackgroundTasks {
                self_waking: false,
                native_idle: false,
                tasks: vec![task],
                native_wake: None,
            }),
        )
        .await;
    common::eventually(
        "the late native history report to be acknowledged",
        || async { world.commands.acknowledged(world.machine_id, &stream) > through_loss },
    )
    .await;
    let lost: bool = sqlx::query_scalar(r#"SELECT payload @? '$.background_tasks.*.task.state ? (@ == "lost")' FROM items WHERE thread_id=$1 AND native_ref='background-command'"#)
        .bind(thread).fetch_one(world.pool.pool()).await.unwrap();
    assert!(lost);
    assert_eq!(world.turn_starts(thread).len(), 1);
}

#[tokio::test]
async fn a_confirmed_exact_end_wins_over_a_later_natural_completion_without_starting_a_turn() {
    use orch_authz::Principal;
    use orch_engine::background::Identity;
    use orch_link_contract::background::{
        BackgroundEndState, BackgroundPlace, BackgroundState, NativeBackground,
        NativeBackgroundCommand, NativeBackgroundEnd,
    };
    use orch_link_contract::commands::CommandResult;
    let world = World::new().await;
    let thread = world.thread("Exact task End").await;
    let (first, _) = world.running_turn(thread, "Begin").await;
    let task = command_task(&world, thread, first.turn_id).await;
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    let command = json!([task.id, task.run_id]).to_string();
    let id = command.clone();
    let origin = first.turn_id;
    world
        .machine
        .handler
        .on(CommandKind::SessionBackground, move |_| {
            Ok(CommandResult::SessionBackground(NativeBackground {
                supported: true,
                as_of: None,
                state: BackgroundState::Live,
                reason: None,
                next_cursor: None,
                total: Some(1),
                items: vec![NativeBackgroundCommand {
                    id: id.clone(),
                    native_item_ref: Some("background-command".into()),
                    turn_id: Some(origin),
                    command: Some("Build".into()),
                    command_truncated: false,
                    cwd: None,
                    place: BackgroundPlace::Environment,
                    since: None,
                    cpu_percent: None,
                    memory_bytes: None,
                }],
            }))
        });
    world
        .machine
        .handler
        .on(CommandKind::SessionBackgroundEnd, move |command| {
            let Command::SessionBackgroundEnd(body) = command else {
                panic!("exact End")
            };
            Ok(CommandResult::SessionBackgroundEnd(NativeBackgroundEnd {
                id: body.command_id.clone(),
                state: BackgroundEndState::Ended,
                reason: None,
            }))
        });
    let credential = orch_types::new_id();
    sqlx::query("INSERT INTO sessions(id,user_id,refresh_hash,expires_at) VALUES($1,$2,$3,$4)")
        .bind(credential)
        .bind(world.ada)
        .bind(vec![7_u8; 32])
        .bind(world.clock.now() + chrono::Duration::hours(1))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let caller = Caller::session(world.ada);
    let identity = Identity {
        credential_id: credential,
        token: false,
    };
    let roster = world
        .engine()
        .background(&caller, identity, thread, None, None)
        .await
        .unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    let principal = Principal::load(&mut tx, caller, world.clock.time())
        .await
        .unwrap();
    let job = world
        .engine()
        .end_background_guarded(
            tx,
            principal,
            identity,
            thread,
            orch_api_contract::background::BackgroundEndRequest::Selected(
                orch_api_contract::background::BackgroundEndSelected {
                    list_ref: roster.list_ref.unwrap(),
                    command_ids: vec![roster.items[0].id.clone()],
                },
            ),
        )
        .await
        .unwrap();
    common::eventually("the exact End to commit", || async {
        world
            .scalar::<String>("SELECT state FROM jobs WHERE id=$1", job.id)
            .await
            == "succeeded"
    })
    .await;
    native_finished(&world, thread, task).await;
    let stopped: bool = sqlx::query_scalar(r#"SELECT payload @? '$.background_tasks.*.task.state ? (@ == "stopped")' FROM items WHERE thread_id=$1 AND native_ref='background-command'"#)
        .bind(thread).fetch_one(world.pool.pool()).await.unwrap();
    assert!(stopped);
    assert_eq!(world.turn_starts(thread).len(), 1);
}

#[tokio::test]
async fn different_authors_can_steer_or_interrupt_while_background_work_runs() {
    for delivery in [
        orch_api_contract::threads::Delivery::Steer,
        orch_api_contract::threads::Delivery::Interrupt,
    ] {
        let world = World::new().await;
        let thread = world.thread("Shared background session").await;
        let (first, _) = world.running_turn(thread, "Begin").await;
        command_task(&world, thread, first.turn_id).await;
        let person = world.person("reviewer").await;
        world.give(person, "interactor").await;
        let mut input = queued("Continue while the earlier work runs");
        input.delivery = delivery;
        let submitted = world
            .engine()
            .submit_input(&Caller::session(person), thread, input)
            .await
            .unwrap();
        assert_ne!(
            submitted.input.state_detail.as_deref(),
            Some("background_author")
        );
        let command = if delivery == orch_api_contract::threads::Delivery::Steer {
            CommandKind::TurnSteer
        } else {
            CommandKind::TurnInterrupt
        };
        world.wait_ran(command, 1).await;
        assert_eq!(
            submitted.input.author.as_ref().map(|author| author.id),
            Some(person)
        );
        if delivery == orch_api_contract::threads::Delivery::Steer {
            let charge: Option<orch_types::Id> = world
                .scalar(
                    "SELECT background_charge_to FROM turns WHERE id=$1",
                    first.turn_id,
                )
                .await;
            assert_eq!(charge, Some(world.ada));
        }
    }
}

fn overlap_request(
    turn: orch_types::Id,
    model: &str,
    id: &str,
    tokens: i64,
    child: bool,
) -> orch_link_contract::events::Event {
    use orch_agent_contract::requests::{RequestTokens, UsageRequest};
    let mut request = UsageRequest {
        id: id.into(),
        model: Some(model.into()),
        subagent: child,
        tokens: RequestTokens {
            input_tokens: Some(tokens),
            cached_input_tokens: Some(0),
            cache_write_tokens: Some(0),
            output_tokens: Some(0),
            reasoning_tokens: None,
            total_tokens: Some(tokens),
        },
        ..Default::default()
    };
    request.finish("done", None, None);
    orch_link_contract::events::Event::ModelRequest(orch_link_contract::events::ModelRequest {
        turn_id: turn,
        request,
    })
}

async fn request_recorded(
    world: &World,
    turn: orch_types::Id,
    reference: &str,
) -> orch_db::usage::ledger::EventRow {
    common::eventually("the actual request report to be counted", || async {
        world
            .ledger_of(turn)
            .await
            .iter()
            .any(|row| row.request_ref.as_deref() == Some(reference))
    })
    .await;
    world
        .ledger_of(turn)
        .await
        .into_iter()
        .find(|row| row.request_ref.as_deref() == Some(reference))
        .unwrap()
}

#[tokio::test]
async fn overlapping_authors_keep_their_authority_and_charge_native_and_forwarded_usage_to_the_first_owner()
 {
    use orch_link_contract::events::{Event, LeaseUsage};
    for forwarded in [false, true] {
        let world = if forwarded {
            World::on_a_key("sk-orch-overlap-fixture").await
        } else {
            World::new().await
        };
        let thread = world.thread("Overlap accounting").await;
        let bea = world.person("bea").await;
        let carl = world.person("carl").await;
        world.give(bea, "interactor").await;
        world.give(carl, "interactor").await;
        let submitted = world
            .engine()
            .submit_input(&Caller::session(bea), thread, queued("Run background work"))
            .await
            .unwrap();
        let first = world.turn_start(thread, 0).await;
        world.say(thread, common::turn_began(first.turn_id)).await;
        world.wait_turn(first.turn_id, "running").await;
        let task = command_task(&world, thread, first.turn_id).await;
        let original_segment = world.segment_of(thread).await;
        let lease = if forwarded {
            Some(world.open_lease_of(thread).await)
        } else {
            None
        };
        let usage = |lease, reference: &str, tokens| {
            Event::LeaseUsage(LeaseUsage {
                lease_id: lease,
                model: first.model.clone(),
                request_ref: Some(reference.into()),
                timing: None,
                usage: serde_json::from_value(
                    json!({"input_tokens":tokens,"cached_input_tokens":0,"output_tokens":0,
                "reasoning_tokens":0,"total_tokens":tokens}),
                )
                .unwrap(),
            })
        };
        if let Some(lease) = lease {
            world
                .say_of_provider(usage(lease, "before-overlap", 20))
                .await;
        } else {
            world
                .say(
                    thread,
                    overlap_request(first.turn_id, &first.model, "before-overlap", 20, false),
                )
                .await;
        }
        let before = request_recorded(&world, first.turn_id, "before-overlap").await;
        assert!(before.counted);
        assert_eq!(before.user_id, Some(bea));
        world
            .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
            .await;
        world.wait_state(thread, "idle").await;
        let next_input = world
            .engine()
            .submit_input(&Caller::session(carl), thread, queued("Work alongside it"))
            .await
            .unwrap();
        let second = world.turn_start(thread, 1).await;
        world.say(thread, common::turn_began(second.turn_id)).await;
        world.wait_turn(second.turn_id, "running").await;
        assert_ne!(
            next_input.input.state_detail.as_deref(),
            Some("background_author")
        );
        assert_eq!(world.segment_of(thread).await, original_segment);
        assert_eq!(session_open_count(&world, false), 1);
        assert_eq!(session_open_count(&world, true), usize::from(forwarded));
        let authors:Vec<(orch_types::Id,Option<orch_types::Id>,bool,Option<orch_types::Id>)>=sqlx::query_as(
            "SELECT t.input_id,q.author_id,t.background_overlap,t.background_charge_to FROM turns t JOIN thread_inputs q ON q.id=t.input_id WHERE t.id=ANY($1) ORDER BY t.ordinal")
            .bind(vec![first.turn_id,second.turn_id]).fetch_all(world.pool.pool()).await.unwrap();
        assert_eq!(
            authors,
            vec![
                (submitted.input.id, Some(bea), true, Some(world.ada)),
                (next_input.input.id, Some(carl), true, Some(world.ada))
            ]
        );
        let late = overlap_request(first.turn_id, &first.model, "overlap-child", 30, true);
        world.say(thread, late.clone()).await;
        world.say(thread, late).await;
        let late = request_recorded(&world, first.turn_id, "overlap-child").await;
        assert_eq!(late.user_id, Some(world.ada));
        assert_eq!(late.counted, !forwarded);
        let current = if let Some(lease) = lease {
            assert_eq!(world.open_lease_of(thread).await, lease);
            let mut conn = world.pool.acquire().await.unwrap();
            assert_eq!(
                orch_db::usage::ledger::attribution_of_lease(&mut conn, lease)
                    .await
                    .unwrap()
                    .unwrap()
                    .user_id,
                Some(world.ada)
            );
            usage(lease, "overlap-main", 40)
        } else {
            overlap_request(second.turn_id, &second.model, "overlap-main", 40, false)
        };
        if forwarded {
            world.say_of_provider(current.clone()).await;
            world.say_of_provider(current).await;
        } else {
            world.say(thread, current.clone()).await;
            world.say(thread, current).await;
        }
        let current = request_recorded(&world, second.turn_id, "overlap-main").await;
        assert_eq!(current.user_id, Some(world.ada));
        assert!(current.counted);
        assert_eq!(current.total_tokens, 40);
        // Already counted rows keep their original author when the shared phase begins.
        assert_eq!(
            request_recorded(&world, first.turn_id, "before-overlap")
                .await
                .user_id,
            Some(bea)
        );
        world
            .say(thread, turn_ended(second.turn_id, TurnOutcome::Completed))
            .await;
        world.wait_state(thread, "idle").await;
        native_finished(&world, thread, task).await;
        let wake = world.turn_start(thread, 2).await;
        let (wake_author,charged):(Option<orch_types::Id>,Option<orch_types::Id>)=sqlx::query_as(
            "SELECT q.author_id,t.background_charge_to FROM turns t JOIN thread_inputs q ON q.id=t.input_id WHERE t.id=$1")
            .bind(wake.turn_id).fetch_one(world.pool.pool()).await.unwrap();
        assert_eq!(wake_author, Some(bea));
        assert_eq!(charged, Some(world.ada));
        assert_eq!(
            world
                .ledger_of(second.turn_id)
                .await
                .iter()
                .filter(|row| row.request_ref.as_deref() == Some("overlap-main") && row.counted)
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn same_author_background_continuations_keep_ordinary_billing_and_model_refusals_do_not_start_an_overlap()
 {
    let world = World::new().await;
    let thread = world.thread("One background author").await;
    let bea = world.person("bea").await;
    let carl = world.person("carl").await;
    world.give(bea, "interactor").await;
    world.give(carl, "interactor").await;
    world
        .engine()
        .submit_input(&Caller::session(bea), thread, queued("Begin"))
        .await
        .unwrap();
    let first = world.turn_start(thread, 0).await;
    world.say(thread, common::turn_began(first.turn_id)).await;
    world.wait_turn(first.turn_id, "running").await;
    command_task(&world, thread, first.turn_id).await;
    world.access.deny(carl);
    let refused = world
        .engine()
        .submit_input(&Caller::session(carl), thread, steering("No model access"))
        .await
        .unwrap();
    assert_eq!(
        refused.input.state_detail.as_deref(),
        Some("model_use_required")
    );
    assert_eq!(world.seen.of_kind(CommandKind::TurnSteer).len(), 0);
    assert!(
        !world
            .scalar::<bool>(
                "SELECT background_overlap FROM turns WHERE id=$1",
                first.turn_id
            )
            .await
    );
    world
        .engine()
        .submit_input(&Caller::session(bea), thread, steering("Keep going"))
        .await
        .unwrap();
    world.wait_ran(CommandKind::TurnSteer, 1).await;
    world
        .say(
            thread,
            overlap_request(first.turn_id, &first.model, "same-author", 10, false),
        )
        .await;
    assert_eq!(
        request_recorded(&world, first.turn_id, "same-author")
            .await
            .user_id,
        Some(bea)
    );
    assert!(
        !world
            .scalar::<bool>(
                "SELECT background_overlap FROM turns WHERE id=$1",
                first.turn_id
            )
            .await
    );
}

#[tokio::test]
async fn late_child_and_continuation_usage_keep_the_original_author_and_count_replays_once() {
    use orch_agent_contract::requests::{RequestTokens, UsageRequest};
    use orch_link_contract::events::{Event, ModelRequest};
    let world = World::new().await;
    let thread = world.thread("Owned continuation usage").await;
    let (first, _) = world.running_turn(thread, "Begin").await;
    let task = command_task(&world, thread, first.turn_id).await;
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    let mut request = UsageRequest {
        id: "late-child-request".into(),
        model: Some(first.model.clone()),
        subagent: true,
        tokens: RequestTokens {
            input_tokens: Some(80),
            cached_input_tokens: Some(0),
            cache_write_tokens: Some(0),
            output_tokens: Some(20),
            reasoning_tokens: None,
            total_tokens: Some(100),
        },
        ..Default::default()
    };
    request.finish("done", None, None);
    let event = Event::ModelRequest(ModelRequest {
        turn_id: first.turn_id,
        request: request.clone(),
    });
    world.say(thread, event.clone()).await;
    world.say(thread, event).await;
    let first_usage = world.wait_ledger(first.turn_id, 1).await;
    assert_eq!(
        first_usage
            .iter()
            .filter(|row| row.counted)
            .map(|row| row.total_tokens)
            .sum::<i64>(),
        100
    );
    native_finished(&world, thread, task).await;
    let continuation = world.turn_start(thread, 1).await;
    world
        .say(thread, common::turn_began(continuation.turn_id))
        .await;
    request.id = "completion-request".into();
    request.subagent = false;
    let event = Event::ModelRequest(ModelRequest {
        turn_id: continuation.turn_id,
        request,
    });
    world.say(thread, event.clone()).await;
    world.say(thread, event).await;
    let next_usage = world.wait_ledger(continuation.turn_id, 1).await;
    assert_eq!(
        next_usage
            .iter()
            .filter(|row| row.counted)
            .map(|row| row.total_tokens)
            .sum::<i64>(),
        100
    );
    assert!(
        first_usage
            .iter()
            .chain(next_usage.iter())
            .all(|row| row.user_id == Some(world.ada))
    );
}

#[tokio::test]
async fn lifecycle_changes_require_reviewing_live_tasks_before_the_session_can_be_closed() {
    let world = World::new().await;
    let thread = world.thread("Reviewed lifecycle").await;
    let (first, _) = world.running_turn(thread, "Begin").await;
    command_task(&world, thread, first.turn_id).await;
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    let mut tx = world.pool.begin().await.unwrap();
    let error = world
        .engine()
        .lock_archive_in(&mut tx, thread)
        .await
        .unwrap_err();
    assert!(matches!(error,orch_engine::EngineError::BackgroundRunning(id) if id==thread));
    tx.rollback().await.unwrap();
    assert_eq!(world.seen.of_kind(CommandKind::SessionClose).len(), 0);
}

#[tokio::test]
async fn a_running_child_request_is_not_finalized_by_its_foreground_turn_ending() {
    use orch_agent_contract::requests::{RequestTokens, UsageRequest};
    use orch_link_contract::background::{
        BackgroundTask, BackgroundTaskKind, BackgroundTaskState, BackgroundTasks,
    };
    use orch_link_contract::events::{Event, ModelRequest};
    let world = World::new().await;
    let thread = world.thread("Child request lifetime").await;
    let (first, _) = world.running_turn(thread, "Begin").await;
    world.say(thread,Event::decode("item.started",json!({"turn_id":first.turn_id,"native_ref":"child-source","stream":"main","parent_native_ref":null,"kind":"tool_call",
        "payload":{"tool":{"kind":"subagent","child_stream":"sub:child","name":"Review","role":null,"prompt":"Review","state":"running","model":null},
        "title":"Review","started_at":world.clock.time(),"duration_ms":null,"raw":{"name":"Agent","input":{"inline":"","total_len":0,"truncated":false,"blob":null},"output":null,"is_error":false}}})).unwrap()).await;
    let child = BackgroundTask {
        id: "child".into(),
        run_id: None,
        native_item_ref: "child-source".into(),
        origin_turn: first.turn_id,
        kind: BackgroundTaskKind::Subagent,
        label: "Review".into(),
        name: Some("Review".into()),
        nickname: None,
        role: None,
        state: BackgroundTaskState::Running,
        exit_code: None,
        can_end: false,
    };
    world
        .say(
            thread,
            Event::BackgroundTasks(BackgroundTasks {
                self_waking: false,
                native_idle: false,
                native_wake: None,
                tasks: vec![child.clone()],
            }),
        )
        .await;
    task_recorded(&world, thread, &child, "running").await;
    let mut request = UsageRequest {
        id: "in-flight-child".into(),
        subagent: true,
        model: Some(first.model.clone()),
        ..Default::default()
    };
    world
        .say(
            thread,
            Event::ModelRequest(ModelRequest {
                turn_id: first.turn_id,
                request: request.clone(),
            }),
        )
        .await;
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(first.turn_id, "completed").await;
    world.wait_followed_up(thread).await;
    let live: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_running WHERE turn_id=$1")
        .bind(first.turn_id)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(
        live, 1,
        "foreground completion is not the end of a child model request"
    );
    request.tokens = RequestTokens {
        input_tokens: Some(80),
        cached_input_tokens: Some(0),
        cache_write_tokens: Some(0),
        output_tokens: Some(20),
        reasoning_tokens: None,
        total_tokens: Some(100),
    };
    request.finish("done", None, None);
    let event = Event::ModelRequest(ModelRequest {
        turn_id: first.turn_id,
        request,
    });
    world.say(thread, event.clone()).await;
    world.say(thread, event).await;
    common::eventually("the child request's terminal report to commit", || async {
        world.ledger_of(first.turn_id).await.iter().any(|row| {
            row.request_ref.as_deref() == Some("in-flight-child")
                && row
                    .timing()
                    .outcome
                    .value()
                    .is_some_and(|outcome| outcome == "done")
        })
    })
    .await;
    let rows = world.ledger_of(first.turn_id).await;
    assert_eq!(
        rows.iter()
            .filter(|row| row.counted)
            .map(|row| row.total_tokens)
            .sum::<i64>(),
        100
    );
    assert!(rows.iter().all(|row| row.user_id == Some(world.ada)));
}

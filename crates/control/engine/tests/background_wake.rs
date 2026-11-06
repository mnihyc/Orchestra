//! Background completion preferences cross the ordinary inherited session boundary.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]
mod common;
use common::{World, queued, steering, turn_ended};
use orch_authz::Caller;
use orch_link_contract::commands::{Command, CommandKind};
use orch_link_contract::events::TurnOutcome;
use serde_json::json;

async fn command_task(world: &World, thread: orch_types::Id, turn: orch_types::Id) -> orch_link_contract::background::BackgroundTask {
    use orch_link_contract::background::{BackgroundTask, BackgroundTaskKind, BackgroundTaskState, BackgroundTasks};
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
        id:"task".into(), run_id:Some("run".into()), native_item_ref:"background-command".into(),
        origin_turn:turn, kind:BackgroundTaskKind::Command, label:"Build".into(),
        state:BackgroundTaskState::Running, exit_code:None, can_end:true,
        name:None, nickname:None, role:None,
    };
    world.say(thread, orch_link_contract::events::Event::BackgroundTasks(BackgroundTasks {
        self_waking:true, tasks:vec![task.clone()], native_wake:None,
    })).await;
    task
}

async fn native_finished(world: &World, thread: orch_types::Id, mut task: orch_link_contract::background::BackgroundTask) {
    use orch_link_contract::background::{BackgroundTaskState, BackgroundTasks, NativeBackgroundWake};
    task.state = BackgroundTaskState::Completed;
    task.exit_code = Some(0);
    let key = json!([task.id, task.run_id]).to_string();
    world.say(thread, orch_link_contract::events::Event::BackgroundTasks(BackgroundTasks {
        self_waking:true, tasks:vec![task], native_wake:Some(NativeBackgroundWake {
            token:"native-followup".into(),task_ids:vec![key],
        }),
    })).await;
}

#[tokio::test]
async fn background_wake_is_on_by_default_and_an_inherited_change_waits_for_session_open() {
    let world = World::new().await;
    let thread = world.thread("Background completion").await;
    let (running, _) = world.running_turn(thread, "Begin").await;
    let opens = world.seen.of_kind(CommandKind::SessionOpen);
    assert!(opens.iter().any(|command| matches!(command, Command::SessionOpen(open) if open.background_wake)));
    world.set(world.topic, "threads.background_wake", json!("off")).await;
    world.engine().submit_input(&Caller::session(world.ada), thread, steering("Keep working")).await.unwrap();
    world.wait_ran(CommandKind::TurnSteer, 1).await;
    assert_eq!(world.seen.of_kind(CommandKind::SessionOpen).len(), opens.len());
    world.say(thread, turn_ended(running.turn_id, TurnOutcome::Completed)).await;
    world.wait_state(thread, "idle").await;
    world.engine().submit_input(&Caller::session(world.ada), thread, queued("Go on")).await.unwrap();
    world.turn_start(thread, 1).await;
    let opens = world.seen.of_kind(CommandKind::SessionOpen);
    let last = opens.iter().rev().find_map(|command| match command {
        Command::SessionOpen(open) => Some(open),
        _ => None,
    }).unwrap();
    assert!(!last.background_wake);
}

#[tokio::test]
async fn a_native_followup_uses_the_original_author_and_records_its_cause_before_output() {
    let world = World::new().await;
    let thread = world.thread("Native completion").await;
    let (first, submitted) = world.running_turn(thread, "Begin").await;
    let task = command_task(&world, thread, first.turn_id).await;
    world.say(thread, turn_ended(first.turn_id, TurnOutcome::Completed)).await;
    world.wait_state(thread, "idle").await;
    native_finished(&world, thread, task).await;
    let next = world.turn_start(thread, 1).await;
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
    world.say(thread, turn_ended(first.turn_id, TurnOutcome::Completed)).await;
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
    world.engine().submit_input(&Caller::session(author), thread, queued("Begin")).await.unwrap();
    let first = world.turn_start(thread, 0).await;
    world.say(thread, common::turn_began(first.turn_id)).await;
    world.wait_turn(first.turn_id, "running").await;
    let task = command_task(&world, thread, first.turn_id).await;
    world.say(thread, turn_ended(first.turn_id, TurnOutcome::Completed)).await;
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
    let item: orch_types::Id = sqlx::query_scalar("SELECT id FROM items WHERE thread_id=$1 AND native_ref='background-command'")
        .bind(thread).fetch_one(world.pool.pool()).await.unwrap();
    world.say(thread, turn_ended(first.turn_id, TurnOutcome::Completed)).await;
    world.wait_state(thread, "idle").await;
    task.state = BackgroundTaskState::Failed;
    task.exit_code = Some(2);
    let report = BackgroundTasks { self_waking:false, tasks:vec![task], native_wake:None };
    world.say(thread, orch_link_contract::events::Event::BackgroundTasks(report.clone())).await;
    let next = world.turn_start(thread, 1).await;
    assert_eq!(next.native_wake, None);
    assert_eq!(next.text, format!("Background task \"Build\" finished (exit 2). Its output is in the item {item}."));
    world.say(thread, orch_link_contract::events::Event::BackgroundTasks(report)).await;
    let inputs: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1 AND source='system'")
        .bind(thread).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(inputs, 1, "replayed completion cannot reserve a second continuation");
}

#[tokio::test]
async fn a_late_child_completion_starts_a_controller_turn_with_its_reported_name() {
    use orch_link_contract::background::{BackgroundTask, BackgroundTaskKind, BackgroundTaskState, BackgroundTasks};
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
        id:"child".into(),run_id:None,native_item_ref:"spawn".into(),origin_turn:first.turn_id,
        kind:BackgroundTaskKind::Subagent,label:"Review changes".into(),
        name:Some("Review changes".into()),nickname:Some("Finch".into()),role:Some("reviewer".into()),
        state:BackgroundTaskState::Running,exit_code:None,can_end:false,
    };
    world.say(thread, orch_link_contract::events::Event::BackgroundTasks(BackgroundTasks {
        self_waking:false,tasks:vec![task.clone()],native_wake:None,
    })).await;
    world.say(thread, turn_ended(first.turn_id, TurnOutcome::Completed)).await;
    world.wait_state(thread, "idle").await;
    task.state = BackgroundTaskState::Completed;
    world.say(thread, orch_link_contract::events::Event::BackgroundTasks(BackgroundTasks {
        self_waking:false,tasks:vec![task],native_wake:None,
    })).await;
    let next = world.turn_start(thread, 1).await;
    assert_eq!(next.text, "Subagent \"Review changes\" finished.");
    assert_eq!(next.native_wake, None);
}

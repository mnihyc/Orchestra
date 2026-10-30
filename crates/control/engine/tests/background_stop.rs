//! An interrupted turn leaves a live command's line open until the harness reports its actual exit.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, eventually, turn_ended};
use orch_agent_contract::events::{OutputRef, RawCall, ToolCallPayload, ToolPayload};
use orch_link_contract::events::{
    EmittedItemKind, Event, ItemCompleted, ItemStarted, ItemUpdated, TurnOutcome,
};
use orch_link_contract::foreign::{ItemPayload, ItemStatus};
use orch_types::{Id, Json};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_cleanup_changes_only_its_stopped_turn_and_preserves_the_queue_hold() {
    let world = World::new().await;
    let thread = world.thread("Late native cleanup").await;
    let (start, _) = world.running_turn(thread, "Run").await;
    let caller = orch_authz::Caller::session(world.ada);
    world
        .engine()
        .submit_input(
            &caller,
            thread,
            orch_engine::inputs::InputRequest {
                plan: false,
                content: vec![orch_api_contract::threads::ContentPart::Text {
                    text: "Waiting".into(),
                }],
                delivery: orch_api_contract::threads::Delivery::Queue,
                client_id: "late-cleanup-waiting".into(),
                bypass: None,
            },
        )
        .await
        .unwrap();
    world
        .engine()
        .interrupt(&caller, thread, start.turn_id)
        .await
        .unwrap();
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Interrupted))
        .await;
    world.wait_turn(start.turn_id, "interrupted").await;
    let held = world.sync(thread).await.thread.queue;
    world
        .say(
            thread,
            Event::TurnCommandCleanup(orch_link_contract::events::TurnCommandCleanup {
                turn_id: start.turn_id,
                result: orch_link_contract::background::CommandCleanup {
                    ended: 1,
                    still_running: 1,
                    unknown: 1,
                },
            }),
        )
        .await;
    eventually("late native cleanup is visible after reload", || async {
        world
            .sync(thread)
            .await
            .turns
            .iter()
            .find(|turn| turn.id == start.turn_id)
            .is_some_and(|turn| turn.command_cleanup.is_some())
    })
    .await;
    let sync = world.sync(thread).await;
    let stopped = sync
        .turns
        .iter()
        .find(|turn| turn.id == start.turn_id)
        .unwrap();
    assert_eq!(
        stopped.state,
        orch_api_contract::threads::TurnState::Interrupted
    );
    assert_eq!(stopped.command_cleanup.unwrap().unknown, 1);
    assert_eq!(
        sync.thread.queue, held,
        "cleanup never resumes or changes a Stop-held queue"
    );
}

fn command(background: bool, exit_code: Option<i64>, started_at: orch_types::Time) -> ItemPayload {
    ItemPayload::ToolCall(ToolCallPayload {
        tool: ToolPayload::Command {
            command: "sleep 300".into(),
            cwd: "/workspace".into(),
            exit_code,
            background,
            process_id: Some("22".into()),
            actions: Vec::new(),
            output: OutputRef::of(""),
        },
        title: "Run sleep 300".into(),
        started_at,
        duration_ms: None,
        raw: RawCall {
            name: "commandExecution".into(),
            input: OutputRef::of("sleep 300"),
            output: None,
            is_error: exit_code.is_some_and(|code| code != 0),
        },
    })
}

async fn state(world: &World, thread: Id) -> (String, Json) {
    sqlx::query_as(
        "SELECT status, payload FROM items WHERE thread_id = $1 AND native_ref = 'live-command'",
    )
    .bind(thread)
    .fetch_one(world.pool.pool())
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_interrupted_turn_never_calls_a_live_background_command_cancelled_and_accepts_its_later_exit()
 {
    let world = World::new().await;
    let thread = world.thread("Stop with a live command").await;
    let (start, _) = world.running_turn(thread, "Run sleep 300").await;
    world
        .say(
            thread,
            Event::ItemStarted(ItemStarted {
                turn_id: start.turn_id,
                native_ref: "live-command".into(),
                stream: "main".into(),
                parent_native_ref: None,
                kind: EmittedItemKind::ToolCall,
                payload: command(false, None, world.clock.time()),
            }),
        )
        .await;
    world
        .say(
            thread,
            Event::ItemUpdated(ItemUpdated {
                native_ref: "live-command".into(),
                payload: json!({ "tool": { "background": true } }),
            }),
        )
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Interrupted))
        .await;
    world.wait_turn(start.turn_id, "interrupted").await;
    assert_eq!(
        state(&world, thread).await.0,
        "in_progress",
        "the turn ended but its command is still alive"
    );
    world
        .say(
            thread,
            Event::ItemUpdated(ItemUpdated {
                native_ref: "live-command".into(),
                payload: json!({ "title": "Command may still be running: sleep 300" }),
            }),
        )
        .await;
    eventually(
        "the command's uncertain state is kept after the turn",
        || async {
            state(&world, thread).await.1["title"] == "Command may still be running: sleep 300"
        },
    )
    .await;
    assert_eq!(state(&world, thread).await.0, "in_progress");
    world
        .say(
            thread,
            Event::ItemCompleted(ItemCompleted {
                native_ref: "live-command".into(),
                status: ItemStatus::Failed,
                payload: command(true, Some(-1), world.clock.time()),
                output_blob: None,
            }),
        )
        .await;
    eventually("the command's native exit is kept", || async {
        state(&world, thread).await.0 == "failed"
    })
    .await;
    assert_eq!(state(&world, thread).await.1["tool"]["exit_code"], -1);
}

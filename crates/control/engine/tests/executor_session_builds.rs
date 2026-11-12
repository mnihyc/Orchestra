//! An admitted native executor build is retained in its ticket and refreshed at a session boundary.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

mod common;
mod hosting;

use common::World;
use orch_link_contract::commands::{Command, SessionMode, SessionOpen, TicketSlot};
use orch_types::Id;
use serde_json::json;

fn opens(world: &World, thread: Id) -> Vec<SessionOpen> {
    ["session.open", "session.open.fenced"]
        .into_iter()
        .flat_map(|kind| world.commands.issued_commands(kind))
        .filter_map(
            |(_, cmd)| match Command::decode(&cmd.kind, cmd.body).unwrap() {
                Command::SessionOpen(open) if open.thread_id == thread => Some(open),
                Command::SessionOpenFenced(open) if open.session.thread_id == thread => {
                    Some(*open.session)
                }
                _ => None,
            },
        )
        .collect()
}

fn admitted_build(open: &SessionOpen) -> Option<&str> {
    let Some(TicketSlot::Asked(asked)) = open.exec.ticket.as_ref() else {
        return None;
    };
    asked
        .ticket
        .more
        .get("executor_version")
        .and_then(|version| version.as_str())
}

async fn report_build(world: &World, version: &str) {
    let mut facts = hosting::facts();
    facts["executors"] = json!([{"kind":"codex-exec-server","version":version,"available":true,
        "sandbox_modes":["read_only","workspace","open"]}]);
    sqlx::query(
        "UPDATE environments SET config=jsonb_set(config,'{exec.offers}',$2),facts=$3 WHERE id=$1",
    )
    .bind(world.environment)
    .bind(json!(["codex-exec-server"]))
    .bind(facts)
    .execute(world.pool.pool())
    .await
    .unwrap();
}

#[tokio::test]
async fn a_changed_native_executor_build_reopens_the_same_conversation_once_with_its_new_ticket() {
    let world = World::observed().await;
    let mut descriptor = common::descriptor();
    descriptor["arrangements"] = json!([
        {"type":"separated","executor":"codex-exec-server","executor_version":"*"}
    ]);
    world.declares(descriptor).await;
    report_build(&world, "1.0.0").await;
    let thread = world.thread("Native build tickets").await;
    world.running_turn(thread, "First build").await;
    let first = opens(&world, thread);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].mode, SessionMode::New);
    assert_eq!(admitted_build(&first[0]), Some("1.0.0"));
    let segment = first[0].segment_id;
    world.run_turn(thread, 0, "Done").await;
    let native: String = sqlx::query_scalar("SELECT native_session_ref FROM segments WHERE id=$1")
        .bind(segment)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    report_build(&world, "2.0.0").await;
    world.running_turn(thread, "Second build").await;
    let changed = opens(&world, thread);
    assert_eq!(changed.len(), 2);
    let reopened = changed
        .iter()
        .find(|open| admitted_build(open) == Some("2.0.0"))
        .unwrap();
    assert_eq!(reopened.mode, SessionMode::Resume);
    assert_eq!(reopened.segment_id, segment);
    assert_eq!(
        reopened.native_session_ref.as_deref(),
        Some(native.as_str())
    );
    world.run_turn(thread, 1, "Done").await;
    world.running_turn(thread, "Same second build").await;
    assert_eq!(
        opens(&world, thread).len(),
        2,
        "unchanged builds keep the accepted session"
    );
    assert_eq!(
        world.turn_starts(thread).len(),
        3,
        "only the three authored inputs start turns"
    );
    world.run_turn(thread, 2, "Done").await;
}

//! A session that is open stays known when the server starts again, and one that is opened again says why.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_that_is_still_open_is_not_opened_again_when_the_server_starts_again() {
    let world = World::new().await;
    let thread = world.thread("Kept session").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&ada, thread, queued("First"))
        .await
        .unwrap();
    let first = world.run_turn(thread, 0, "One.").await;
    world.wait_state(thread, "idle").await;
    assert_eq!(world.ran(CommandKind::SessionOpen), 1);
    let kept: Option<serde_json::Value> = world
        .scalar(
            "SELECT session_hints FROM segments WHERE id = $1",
            world.segment_of(thread).await,
        )
        .await;
    assert!(
        kept.is_some(),
        "what the opening was asked is kept with the segment"
    );
    world.restart_api().await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Second"))
        .await
        .unwrap();
    let second = world.turn_start(thread, 1).await;
    assert_ne!(first.turn_id, second.turn_id);
    assert_eq!(
        world.ran(CommandKind::SessionOpen),
        1,
        "the session the harness still holds is not opened again because the server started again"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_that_is_opened_again_for_a_change_says_why_on_the_thread() {
    let world = World::new().await;
    let thread = world.thread("Reopened session").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&ada, thread, queued("First"))
        .await
        .unwrap();
    world.run_turn(thread, 0, "One.").await;
    world.wait_state(thread, "idle").await;
    // A change that only a new session takes.
    world
        .set(
            world.topic,
            "agents.subagents_at_once",
            serde_json::json!(3),
        )
        .await;
    world
        .engine()
        .submit_input(&ada, thread, queued("Second"))
        .await
        .unwrap();
    world.turn_start(thread, 1).await;
    assert_eq!(world.ran(CommandKind::SessionOpen), 2);
    let marked: i64 = world
        .count(
            "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'marker' AND payload->>'marker' = 'session_reopened'",
            thread,
        )
        .await;
    assert_eq!(
        marked, 1,
        "the thread says that the session was opened again"
    );
    let reason: String = world
        .scalar(
            "SELECT payload->'data'->>'reason' FROM items WHERE thread_id = $1 AND kind = 'marker' AND payload->>'marker' = 'session_reopened'",
            thread,
        )
        .await;
    assert_eq!(reason, "limits");
}

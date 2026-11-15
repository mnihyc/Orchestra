//! A restarted server verifies a retained native session once; unchanged later turns keep that live session.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restarted_server_verifies_the_retained_session_once_and_later_turns_keep_it() {
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
    let segment = world.segment_of(thread).await;
    let native: String = world
        .scalar(
            "SELECT native_session_ref FROM segments WHERE id=$1",
            segment,
        )
        .await;
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
        2,
        "the restarted server verifies the retained native conversation through resume"
    );
    assert_eq!(world.segment_of(thread).await, segment);
    assert_eq!(
        world
            .scalar::<String>(
                "SELECT native_session_ref FROM segments WHERE id=$1",
                segment
            )
            .await,
        native
    );
    world.answer(thread, &second, "second-reply", "Two.").await;
    world
        .say(thread, turn_ended(second.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    world
        .engine()
        .submit_input(&ada, thread, queued("Third"))
        .await
        .unwrap();
    world.turn_start(thread, 2).await;
    assert_eq!(
        world.ran(CommandKind::SessionOpen),
        2,
        "ordinary turns keep the verified session"
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

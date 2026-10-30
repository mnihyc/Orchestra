//! One input on a bound thread becomes a turn: the machine is asked for what the turn needs, and what it says is the conversation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{
    World, message_began, message_done, message_grew, queued, said, turn_began, turn_ended,
};
use orch_api_contract::threads::{ItemKind, ItemPayload, TurnState};
use orch_authz::Caller;
use orch_link_contract::commands::CommandKind;
use orch_link_contract::events::TurnOutcome;
use orch_types::Id;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_input_on_an_idle_thread_starts_a_turn_that_streams_text_and_ends() {
    let world = World::new().await;
    let thread = world.thread("Fix login").await;
    let submitted = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Fix the login page"),
        )
        .await
        .unwrap();
    assert!(submitted.created);

    // The machine is asked for the three things a turn needs, in order.
    let start = world.turn_start(thread, 0).await;
    let kinds: Vec<_> = world
        .seen
        .all()
        .iter()
        .map(orch_link_contract::commands::Command::kind)
        .collect();
    assert_eq!(
        kinds,
        [
            CommandKind::AgentStart,
            CommandKind::SessionOpen,
            CommandKind::TurnStart
        ]
    );
    assert_eq!(common::words_after_where(&start.text), "Fix the login page");
    assert_eq!(start.model, common::MODEL);
    assert_eq!(world.state(thread).await.0, "starting");

    // What the harness then does.
    world.say(thread, turn_began(start.turn_id)).await;
    world.say(thread, message_began(start.turn_id, "m1")).await;
    world.say(thread, message_grew("m1", 0, "I will ")).await;
    world.say(thread, message_grew("m1", 7, "fix.")).await;
    world.say(thread, message_done("m1", "I will fix.")).await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    world.wait_state(thread, "idle").await;
    // What the end of the turn writes after it (its use, who is told) is numbered in the thread too.
    world.wait_followed_up(thread).await;

    // The thread reads back as it happened, numbered in the order it was made.
    let sync = world
        .engine()
        .sync(thread, Some(world.ada), None, 100)
        .await
        .unwrap();
    assert_eq!(
        said(&sync),
        ["user: Fix the login page", "agent: I will fix."]
    );
    let numbers: Vec<_> = sync.items.iter().map(|item| item.seq_created).collect();
    assert!(
        numbers.windows(2).all(|pair| pair[0] < pair[1]),
        "items are numbered in the order they were made: {numbers:?}"
    );
    let ItemPayload::AgentMessage(message) = &sync
        .items
        .iter()
        .find(|item| item.kind() == ItemKind::AgentMessage)
        .unwrap()
        .payload
    else {
        panic!("an agent message")
    };
    assert_eq!(
        (message.text.as_str(), message.text_len),
        ("I will fix.", 11)
    );
    assert_eq!(sync.turns.len(), 1);
    assert_eq!(sync.turns[0].state, TurnState::Completed);
    assert!(
        sync.inputs.is_empty(),
        "an input that was delivered is not among those that wait"
    );
    assert_eq!(
        world.live.deltas().len(),
        2,
        "the text was forwarded as it came"
    );
    let highest: i64 = world
        .scalar(
            "SELECT next_seq - 1 FROM threads WHERE node_id = $1",
            thread,
        )
        .await;
    assert_eq!(
        sync.seq, highest,
        "the sync is as far as the thread's sequence is"
    );
    assert_eq!(sync.thread.seq, highest);
    let last: Id = world
        .scalar("SELECT id FROM turns WHERE thread_id = $1", thread)
        .await;
    assert_eq!(last, start.turn_id);
}

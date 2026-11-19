//! Inputs become turns one after another: queueing, steering, interrupting and the forced end of a turn.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{
    World, eventually, interrupting, meanwhile, message_done, queued, said, steering, turn_ended,
};
use orch_api_contract::threads::{InputState, TurnState};
use orch_authz::Caller;
use orch_engine::inputs::InputError;
use orch_link_contract::commands::{Command, CommandKind};
use orch_link_contract::events::TurnOutcome;
use std::time::Duration;

/// What the next input is told of a turn that ada stopped (T8), once.
const STOPPED_BY_ADA: &str =
    "<orch>Your last turn was stopped by ada (@ada). What it was doing may be half done.</orch>";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_queued_input_starts_the_next_turn_when_the_first_one_ends() {
    let world = World::new().await;
    let thread = world.thread("Two inputs").await;
    let ada = Caller::session(world.ada);
    let first = world
        .engine()
        .submit_input(&ada, thread, queued("First"))
        .await
        .unwrap();
    let second = world
        .engine()
        .submit_input(&ada, thread, queued("Second"))
        .await
        .unwrap();
    assert!(first.created && second.created);
    assert!(
        second.seq > first.seq,
        "every accepted input is a step of the thread's sequence"
    );

    // While the first runs the second waits, and the machine is asked for one turn only.
    let one = world.turn_start(thread, 0).await;
    assert_eq!(common::words_after_where(&one.text), "First");
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(world.turn_starts(thread).len(), 1);
    assert_eq!(world.input_state(second.input.id).await.0, "pending");
    world.answer(thread, &one, "m1", "One.").await;
    world
        .say(thread, turn_ended(one.turn_id, TurnOutcome::Completed))
        .await;

    // The second starts the next turn, with nobody asking for it.
    let two = world.turn_start(thread, 1).await;
    assert_eq!(common::words_after_where(&two.text), "Second");
    assert_ne!(one.turn_id, two.turn_id);
    world.answer(thread, &two, "m2", "Two.").await;
    world
        .say(thread, turn_ended(two.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;

    // The conversation reads in the order it happened, and the harness was set up once.
    let sync = world.sync(thread).await;
    assert_eq!(
        said(&sync),
        ["user: First", "agent: One.", "user: Second", "agent: Two."]
    );
    let ordinals: Vec<_> = sync
        .turns
        .iter()
        .map(|turn| (turn.ordinal, turn.state))
        .collect();
    assert_eq!(
        ordinals,
        [(1, TurnState::Completed), (2, TurnState::Completed)]
    );
    assert_eq!(world.ran(CommandKind::AgentStart), 1);
    assert_eq!(
        world.ran(CommandKind::SessionOpen),
        1,
        "the session is kept between turns"
    );
    assert_eq!(world.ran(CommandKind::TurnStart), 2);
    for input in [first.input.id, second.input.id] {
        assert_eq!(world.input_state(input).await.0, "delivered");
    }
}

#[test]
fn a_second_queued_input_runs_with_default_worker_stacks_even_if_the_suite_stack_is_larger() {
    common::on_default_stack("a_second_queued_input_starts_the_next_turn_when_the_first_one_ends");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steer_reaches_the_turn_that_runs_and_its_words_join_that_turn() {
    let world = World::new().await;
    let thread = world.thread("Steering").await;
    let (start, _) = world.running_turn(thread, "Fix login").await;
    let ada = Caller::session(world.ada);

    let steer = world
        .engine()
        .submit_input(&ada, thread, steering("Also the footer"))
        .await
        .unwrap();
    assert_eq!(
        steer.input.state,
        InputState::Delivering,
        "it goes into the turn that runs, not behind it"
    );
    world.wait_ran(CommandKind::TurnSteer, 1).await;
    let Command::TurnSteer(body) = world.seen.of_kind(CommandKind::TurnSteer)[0].clone() else {
        panic!("a turn.steer")
    };
    assert_eq!(
        (body.turn_id, body.text.as_str()),
        (start.turn_id, "Also the footer")
    );
    world.wait_input(steer.input.id, "delivered").await;

    let sync = world.sync(thread).await;
    let users: Vec<_> = sync
        .items
        .iter()
        .filter(|item| item.kind() == orch_api_contract::threads::ItemKind::UserMessage)
        .map(|item| item.turn_id)
        .collect();
    assert_eq!(
        users,
        [Some(start.turn_id), Some(start.turn_id)],
        "both are in the one turn"
    );
    assert_eq!(
        said(&sync),
        ["user: Fix login", "agent: Working", "user: Also the footer"],
        "the steer is told where it happened: after what the agent had said by then"
    );
    assert_eq!(
        world.turn_starts(thread).len(),
        1,
        "a steer never makes a turn of its own"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steer_uses_the_running_turns_model_permission() {
    let world = World::new().await;
    let thread = world.thread("Running model access").await;
    world.running_turn(thread, "Begin").await;
    world
        .set(thread, "agent.model", serde_json::json!("next-model"))
        .await;
    world
        .set(
            world.topic,
            "thread.inputs_without_use",
            serde_json::json!("refused"),
        )
        .await;
    let caller = Caller::session(world.ada);

    world.access.deny_model(world.ada, common::MODEL);
    let refused = world
        .engine()
        .submit_input(&caller, thread, steering("No access to this turn"))
        .await;
    assert!(matches!(refused, Err(InputError::Denied(_))));
    assert_eq!(world.commands.issued_of("turn.steer"), 0);

    world.access.allow_model(world.ada, common::MODEL);
    world.access.deny_model(world.ada, "next-model");
    let allowed = world
        .engine()
        .submit_input(&caller, thread, steering("Access to this turn"))
        .await
        .unwrap();
    world.wait_input(allowed.input.id, "delivered").await;
    let Command::TurnSteer(body) = world.seen.of_kind(CommandKind::TurnSteer)[0].clone() else {
        panic!("a turn.steer")
    };
    assert_eq!(body.text, "Access to this turn");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn converting_a_queued_input_to_a_steer_checks_its_running_model() {
    let world = World::new().await;
    let thread = world.thread("Queued model access").await;
    world.running_turn(thread, "Begin").await;
    world
        .set(thread, "agent.model", serde_json::json!("next-model"))
        .await;
    let caller = Caller::session(world.ada);
    let waiting = world
        .engine()
        .submit_input(&caller, thread, queued("Queued words"))
        .await
        .unwrap();
    world.access.deny_model(world.ada, common::MODEL);
    let mut tx = world.pool.begin().await.unwrap();
    let principal = orch_authz::Principal::load(&mut tx, caller.clone(), world.clock.time())
        .await
        .unwrap();
    let refused = world
        .engine()
        .update_input_guarded(
            tx,
            principal,
            thread,
            waiting.input.id,
            orch_api_contract::threads::ThreadInputUpdateBody {
                delivery: Some(orch_api_contract::threads::Delivery::Steer),
                ..Default::default()
            },
        )
        .await;
    assert!(matches!(refused, Err(InputError::Denied(_))));
    assert_eq!(world.input_state(waiting.input.id).await.0, "pending");
    assert_eq!(world.commands.issued_of("turn.steer"), 0);

    world.access.allow_model(world.ada, common::MODEL);
    world.access.deny_model(world.ada, "next-model");
    let mut tx = world.pool.begin().await.unwrap();
    let principal = orch_authz::Principal::load(&mut tx, caller, world.clock.time())
        .await
        .unwrap();
    world
        .engine()
        .update_input_guarded(
            tx,
            principal,
            thread,
            waiting.input.id,
            orch_api_contract::threads::ThreadInputUpdateBody {
                delivery: Some(orch_api_contract::threads::Delivery::Steer),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    world.wait_input(waiting.input.id, "delivered").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steer_rechecks_model_use_before_dispatch_after_a_restart() {
    let world = World::new().await;
    let thread = world.thread("Steer access changed").await;
    world.running_turn(thread, "Begin").await;
    world.engine().stop();
    let input = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, steering("Later words"))
        .await
        .unwrap();
    assert_eq!(input.input.state, InputState::Delivering);
    world.access.deny_model(world.ada, common::MODEL);
    world.restart_api().await;
    world.wait_input(input.input.id, "pending").await;
    assert_eq!(world.commands.issued_of("turn.steer"), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_that_comes_before_the_steer_is_acknowledged_never_stands_above_its_words() {
    let world = World::new().await;
    let thread = world.thread("Steered and answered at once").await;
    let (start, _) = world.running_turn(thread, "Fix login").await;
    let ada = Caller::session(world.ada);

    // The machine takes the steer but has not said so when the agent already answers it.
    world.machine.handler.hold(CommandKind::TurnSteer);
    let steer = world
        .engine()
        .submit_input(&ada, thread, steering("Also the footer"))
        .await
        .unwrap();
    world.wait_began_or_issued("turn.steer").await;
    world
        .say(thread, common::message_began(start.turn_id, "m2"))
        .await;
    world.say(thread, message_done("m2", "Footer done.")).await;
    world.machine.handler.release();
    world.wait_input(steer.input.id, "delivered").await;
    // The acknowledgement comes back with the command and the answer on the session's stream, which the engine
    // may take in a moment later: the answer is waited for before the order is read.
    eventually("the answer is taken in", || async {
        world
            .count(
                "SELECT count(*) FROM items WHERE thread_id = $1 AND native_ref = 'm2' AND status = 'completed'",
                thread,
            )
            .await
            == 1
    })
    .await;

    let sync = world.sync(thread).await;
    assert_eq!(
        said(&sync),
        [
            "user: Fix login",
            "agent: Working",
            "user: Also the footer",
            "agent: Footer done."
        ],
        "the words were in their place before they were sent"
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'user_message' AND status = 'completed'",
                thread,
            )
            .await,
        2,
        "each input's words once, confirmed where they stand"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steer_unconfirmed_when_its_turn_ends_returns_to_the_queue() {
    let world = World::new().await;
    let thread = world.thread("Steer after a turn ends").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    world.machine.handler.hold(CommandKind::TurnSteer);
    let steer = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, steering("Next work"))
        .await
        .unwrap();
    world.wait_began_or_issued("turn.steer").await;
    // Keep the supervisor from starting another turn before the returned row is observed.
    world.engine().stop();
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    world.wait_input(steer.input.id, "pending").await;
    assert_eq!(world.input_state(steer.input.id).await.0, "pending");
    world.machine.handler.release();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steer_into_a_thread_with_no_turn_waits_first_and_starts_one() {
    let world = World::new().await;
    let thread = world.thread("Steer on idle").await;
    let ada = Caller::session(world.ada);
    let steer = world
        .engine()
        .submit_input(&ada, thread, steering("Begin with this"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    assert_eq!(common::words_after_where(&start.text), "Begin with this");
    world.answer(thread, &start, "m1", "Done.").await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_input(steer.input.id, "delivered").await;
    assert_eq!(world.ran(CommandKind::TurnSteer), 0);
}

/// How long the engine waits in the tests of a harness that does not answer an interrupt: long enough
/// for a busy machine not to use it up before the test has looked.
const FORCE_AFTER: Duration = Duration::from_secs(4);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stopping_during_setup_keeps_the_delivering_input_unheld_and_fences_the_old_start() {
    let world = World::new().await;
    let thread = world.thread("Stop during setup").await;
    let caller = Caller::session(world.ada);
    world.machine.handler.hold(CommandKind::SessionOpen);
    let submitted = world
        .engine()
        .submit_input(&caller, thread, queued("Do not start yet"))
        .await
        .unwrap();
    world.wait_began(CommandKind::SessionOpen, 1).await;
    let old: orch_types::Id = world
        .scalar(
            "SELECT current_turn_id FROM threads WHERE node_id=$1",
            thread,
        )
        .await;
    world
        .engine()
        .interrupt(&caller, thread, old)
        .await
        .unwrap();
    world.wait_turn(old, "interrupted").await;
    world.wait_input(submitted.input.id, "pending").await;
    let queue = world.sync(thread).await.thread.queue;
    assert!(!queue.held);
    world.machine.handler.release();
    let next = world.turn_start(thread, 0).await;
    assert_ne!(next.turn_id, old);
    assert!(next.text.contains("Do not start yet"));
    world
        .say(thread, turn_ended(next.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_input(submitted.input.id, "delivered").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stopping_a_retried_start_keeps_the_original_input_instead_of_failing_it() {
    let world = World::new().await;
    let thread = world.thread("Stop a retried start").await;
    let caller = Caller::session(world.ada);
    world.refuse(
        CommandKind::TurnStart,
        vec![
            orch_link_contract::commands::CommandError::new(
                orch_link_contract::commands::ErrorCode::Timeout,
                "the first start was not taken",
            )
            .retryable(),
        ],
    );
    let first = world
        .engine()
        .submit_input(&caller, thread, queued("Original work"))
        .await
        .unwrap();
    world.wait_blocked(thread, "error").await;
    assert_eq!(world.input_state(first.input.id).await.0, "pending");

    // Changed instructions reopen the session before the retry can start.
    world
        .set(
            thread,
            "agent.instructions",
            serde_json::json!("New instructions"),
        )
        .await;
    world.machine.handler.hold(CommandKind::SessionOpen);
    let later = world
        .engine()
        .submit_input(&caller, thread, queued("Later work"))
        .await
        .unwrap();
    world.wait_began(CommandKind::SessionOpen, 2).await;
    let retry: orch_types::Id = world
        .scalar(
            "SELECT current_turn_id FROM threads WHERE node_id=$1",
            thread,
        )
        .await;
    world
        .engine()
        .interrupt(&caller, thread, retry)
        .await
        .unwrap();
    world.wait_turn(retry, "interrupted").await;
    assert_eq!(world.input_state(first.input.id).await.0, "pending");
    assert_eq!(world.input_state(later.input.id).await.0, "pending");
    let queue = world.sync(thread).await.thread.queue;
    assert!(queue.held);
    assert_eq!(queue.held_inputs, 1);
    world.machine.handler.release();
}

async fn a_starting_input_survives_a_resource_pause(run_waiting: bool) {
    use orch_access_contract::decisions::ObjectRef;
    use orch_access_contract::permissions::ObjectKind;
    use orch_api_contract::resource_facts::FactTarget;
    use orch_engine::resource_pause::{ResourceResume, ResourceStop};

    let world = World::new().await;
    let thread = world.thread("Pause during setup").await;
    let caller = Caller::session(world.ada);
    world.machine.handler.hold(CommandKind::SessionOpen);
    let submitted = world
        .engine()
        .submit_input(&caller, thread, queued("Original work"))
        .await
        .unwrap();
    world.wait_began(CommandKind::SessionOpen, 1).await;
    let old: orch_types::Id = world
        .scalar(
            "SELECT current_turn_id FROM threads WHERE node_id=$1",
            thread,
        )
        .await;
    let target = FactTarget {
        object: ObjectRef {
            kind: ObjectKind::Provider,
            id: world.provider,
        },
        model_id: None,
    };
    let mut tx = world.pool.begin().await.unwrap();
    let (job, _) = world
        .engine()
        .stop_resource_in(
            &mut tx,
            ResourceStop {
                target: target.clone(),
                name: "Provider",
                by: world.ada,
                note: None,
                deleted: false,
            },
            world.clock.now(),
        )
        .await
        .unwrap();
    sqlx::query("UPDATE providers SET status='disabled' WHERE id=$1")
        .bind(world.provider)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world.engine().stop_resource_now(job.id).await.unwrap();
    world.wait_turn(old, "interrupted").await;
    world.wait_input(submitted.input.id, "pending").await;
    world.machine.handler.release();
    eventually("the resource stop finishes", || async {
        let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1")
            .bind(job.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
        state == "succeeded"
    })
    .await;

    let mut tx = world.pool.begin().await.unwrap();
    sqlx::query("UPDATE providers SET status='ready' WHERE id=$1")
        .bind(world.provider)
        .execute(&mut *tx)
        .await
        .unwrap();
    let finished = world
        .engine()
        .enable_resource_waiting_in(
            &mut tx,
            ResourceResume {
                target: &target,
                name: "Provider",
                by: world.ada,
                run_waiting,
            },
            world.clock.now(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for thread in finished.wakes {
        world.engine().wake(thread);
    }
    if !run_waiting {
        let queue = world.sync(thread).await.thread.queue;
        assert!(queue.held);
        assert_eq!(queue.held_inputs, 1);
        meanwhile(Duration::from_millis(300)).await;
        assert!(world.turn_starts(thread).is_empty());
        world
            .engine()
            .resume_queue(&caller, thread, queue.revision)
            .await
            .unwrap();
    }
    let next = world.turn_start(thread, 0).await;
    assert_ne!(next.turn_id, old);
    assert!(next.text.contains("Original work"));
    let actual: orch_types::Id = sqlx::query_scalar("SELECT input_id FROM turns WHERE id=$1")
        .bind(next.turn_id)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(actual, submitted.input.id);
    world
        .say(thread, turn_ended(next.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_input(submitted.input.id, "delivered").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enabling_a_resource_can_keep_a_starting_input_held_or_run_the_original() {
    a_starting_input_survives_a_resource_pause(false).await;
    a_starting_input_survives_a_resource_pause(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_interrupt_ends_the_turn_when_the_harness_stops() {
    // The harness answers, however late the test gets to say so: the engine waits as long as a
    // deployment's does (thirty seconds), so that the turn is not ended by force on a busy machine.
    let world = World::tuned(|waits| waits.interrupt_force = Duration::from_secs(30)).await;
    let thread = world.thread("Stop it").await;
    let (start, _) = world.running_turn(thread, "Run the whole suite").await;
    let ada = Caller::session(world.ada);

    let turn = world
        .engine()
        .interrupt(&ada, thread, start.turn_id)
        .await
        .unwrap();
    assert!(
        turn.interrupted_by.is_some(),
        "the person who stopped it is on record"
    );
    world.wait_ran(CommandKind::TurnInterrupt, 1).await;
    // The harness stops and says so.
    world.say(thread, message_done("m", "Working")).await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Interrupted))
        .await;
    world.wait_turn(start.turn_id, "interrupted").await;
    world.wait_state(thread, "idle").await;

    let sync = world.sync(thread).await;
    assert_eq!(sync.turns[0].state, TurnState::Interrupted);
    assert!(
        sync.turns[0].error.is_none(),
        "a harness that stopped when asked is no failure"
    );
    assert_eq!(
        sync.turns[0].interrupted_by.as_ref().map(|by| by.id),
        Some(world.ada)
    );
    assert_eq!(
        world.commands.issued_of("session.close"),
        0,
        "its session is kept"
    );
    // Asking again for the turn that has just ended answers it as it ended; nothing waits, so nothing is held.
    let again = world
        .engine()
        .interrupt(&ada, thread, start.turn_id)
        .await
        .unwrap();
    assert_eq!(again.state, TurnState::Interrupted);
    assert!(!world.sync(thread).await.thread.queue.held);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_holds_waiting_and_new_inputs_until_the_seen_queue_is_resumed_in_order() {
    let world = World::tuned(|waits| waits.interrupt_force = Duration::from_secs(30)).await;
    let thread = world.thread("Stop holds").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    let ada = Caller::session(world.ada);
    let one = world
        .engine()
        .submit_input(&ada, thread, queued("One"))
        .await
        .unwrap();
    let two = world
        .engine()
        .submit_input(&ada, thread, queued("Two"))
        .await
        .unwrap();

    // Stop: what waited is held before the harness is asked to stop, and the history says so once.
    world
        .engine()
        .interrupt(&ada, thread, start.turn_id)
        .await
        .unwrap();
    let held = world.sync(thread).await.thread.queue;
    assert!(held.held);
    assert_eq!(held.held_inputs, 2);
    assert_eq!(held.by.as_ref().map(|by| by.id), Some(world.ada));
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Interrupted))
        .await;
    world.wait_turn(start.turn_id, "interrupted").await;
    world.wait_state(thread, "idle").await;
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(
        world.turn_starts(thread).len(),
        1,
        "nothing held starts by itself"
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'marker' AND payload->>'marker' = 'queue_held'",
                thread,
            )
            .await,
        1
    );

    // Sending after Stop appends to the same hold; it cannot overtake One and Two.
    let three = world
        .engine()
        .submit_input(&ada, thread, queued("Three"))
        .await
        .unwrap();
    assert!(three.input.queue_held);
    assert_eq!(three.input.state, InputState::Pending);
    assert!(three.input.turn_id.is_none());
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(
        world.turn_starts(thread).len(),
        1,
        "a new send does not resume the queue"
    );

    // The newer input changed the queue too. Resuming the older view must not silently
    // resume a queue changed since the person last saw it.
    let current = world.sync(thread).await.thread.queue;
    assert!(current.revision > held.revision);
    assert!(current.held);
    assert_eq!(current.held_inputs, 3);
    assert_eq!(current.by, held.by);
    assert_eq!(current.at, held.at);
    let stale = world
        .engine()
        .resume_queue(&ada, thread, held.revision)
        .await;
    assert!(
        matches!(stale, Err(InputError::StaleQueue { revision }) if revision == current.revision),
        "{stale:?}"
    );
    // The one it saw lets the held inputs go, in their order, each its own turn.
    let resumed = world
        .engine()
        .resume_queue(&ada, thread, current.revision)
        .await
        .unwrap();
    assert!(!resumed.held);
    let first = world.run_turn(thread, 1, "Done one.").await;
    assert_eq!(
        common::words_after_where(&first.text),
        format!("{STOPPED_BY_ADA}\n\nOne")
    );
    let second = world.run_turn(thread, 2, "Done two.").await;
    assert_eq!(common::words_after_where(&second.text), "Two");
    let last = world.run_turn(thread, 3, "Done three.").await;
    assert_eq!(common::words_after_where(&last.text), "Three");
    world.wait_state(thread, "idle").await;
    assert_eq!(world.input_state(one.input.id).await.0, "delivered");
    assert_eq!(world.input_state(two.input.id).await.0, "delivered");
    assert_eq!(world.input_state(three.input.id).await.0, "delivered");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_stop_hold_appends_all_new_delivery_modes_and_system_work_without_steering_or_interrupting()
 {
    let world = World::tuned(|waits| waits.interrupt_force = Duration::from_secs(30)).await;
    let thread = world.thread("Held arrivals").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    let ada = Caller::session(world.ada);
    let older = world
        .engine()
        .submit_input(&ada, thread, queued("Older"))
        .await
        .unwrap();
    world
        .engine()
        .interrupt(&ada, thread, start.turn_id)
        .await
        .unwrap();
    world.wait_ran(CommandKind::TurnInterrupt, 1).await;
    let mut ids = vec![older.input.id];
    for request in [
        queued("Queued after Stop"),
        steering("Steer after Stop"),
        interrupting("Interrupt after Stop"),
    ] {
        let wanted = request.delivery;
        let accepted = world
            .engine()
            .submit_input(&ada, thread, request)
            .await
            .unwrap();
        assert_eq!(
            accepted.input.delivery, wanted,
            "the preference is retained without bypassing Stop"
        );
        assert!(accepted.input.queue_held);
        assert_eq!(accepted.input.state, InputState::Pending);
        assert!(accepted.input.turn_id.is_none());
        ids.push(accepted.input.id);
    }
    let notice = world
        .engine()
        .queue_system_input(thread, "system", &queued("System after Stop").content)
        .await
        .unwrap();
    assert!(notice.queue_held);
    ids.push(notice.id);
    let mut sync = world.sync(thread).await;
    let mut waiting: Vec<_> = sync
        .inputs
        .iter()
        .filter(|input| input.state == InputState::Pending)
        .collect();
    waiting.sort_by_key(|input| input.position.parse::<i64>().unwrap());
    assert_eq!(
        waiting.iter().map(|input| input.id).collect::<Vec<_>>(),
        ids
    );
    assert_eq!(sync.thread.queue.held_inputs, 5);
    assert_eq!(world.commands.issued_of("turn.steer"), 0);
    assert_eq!(world.commands.issued_of("turn.interrupt"), 1);
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Interrupted))
        .await;
    world.wait_state(thread, "idle").await;
    assert_eq!(world.turn_starts(thread).len(), 1);
    sync = world.sync(thread).await;
    world
        .engine()
        .resume_queue(&ada, thread, sync.thread.queue.revision)
        .await
        .unwrap();
    for (index, words) in [
        "Older",
        "Queued after Stop",
        "Steer after Stop",
        "Interrupt after Stop",
        "System after Stop",
    ]
    .into_iter()
    .enumerate()
    {
        let started = world.run_turn(thread, index + 1, "Done").await;
        let expected = if index == 0 {
            format!("{STOPPED_BY_ADA}\n\n{words}")
        } else {
            words.to_owned()
        };
        assert_eq!(common::words_after_where(&started.text), expected);
    }
    world.wait_state(thread, "idle").await;
    for input in ids {
        assert_eq!(world.input_state(input).await.0, "delivered");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_racing_a_nonempty_stop_joins_the_hold_whichever_takes_the_thread_lock_first() {
    let world = World::tuned(|waits| waits.interrupt_force = Duration::from_secs(30)).await;
    let thread = world.thread("Stop and send race").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&ada, thread, queued("Already waiting"))
        .await
        .unwrap();
    let engine = world.engine();
    let (stopped, sent) = tokio::join!(
        engine.interrupt(&ada, thread, start.turn_id),
        engine.submit_input(&ada, thread, queued("Racing send")),
    );
    stopped.unwrap();
    let sent = sent.unwrap();
    let sync = world.sync(thread).await;
    assert!(sync.thread.queue.held);
    assert_eq!(sync.thread.queue.held_inputs, 2);
    let row = sync
        .inputs
        .iter()
        .find(|input| input.id == sent.input.id)
        .unwrap();
    assert!(row.queue_held);
    assert_eq!(row.state, InputState::Pending);
    assert!(row.turn_id.is_none());
    assert_eq!(world.turn_starts(thread).len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_of_an_older_turn_while_a_newer_one_runs_changes_nothing_and_names_the_newer() {
    let world = World::new().await;
    let thread = world.thread("Stale stop").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&ada, thread, queued("First"))
        .await
        .unwrap();
    let first = world.run_turn(thread, 0, "Done.").await;
    world.wait_state(thread, "idle").await;
    let (second, _) = world.running_turn(thread, "Second").await;
    let waiting = world
        .engine()
        .submit_input(&ada, thread, queued("Waiting"))
        .await
        .unwrap();

    let stale = world.engine().interrupt(&ada, thread, first.turn_id).await;
    assert!(
        matches!(
            stale,
            Err(orch_engine::turns::InterruptError::NotRunning { current_turn_id: Some(current) })
                if current == second.turn_id
        ),
        "{stale:?}"
    );
    assert_eq!(world.turn_state(second.turn_id).await, "running");
    assert!(!world.sync(thread).await.thread.queue.held);
    assert_eq!(world.input_state(waiting.input.id).await.0, "pending");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_harness_that_does_not_answer_an_interrupt_is_forced_after_the_wait() {
    let clock = std::sync::Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    world.set_config(|waits| waits.interrupt_force = FORCE_AFTER);
    world.restart_api().await;
    let thread = world.thread("Stuck").await;
    let (start, _) = world.running_turn(thread, "Run for a very long time").await;
    let ada = Caller::session(world.ada);
    world.machine.handler.hold(CommandKind::TurnInterrupt);

    world
        .engine()
        .interrupt(&ada, thread, start.turn_id)
        .await
        .unwrap();
    world.wait_began(CommandKind::TurnInterrupt, 1).await;
    // The domain deadline has not advanced, however slowly the machine schedules the harness.
    assert_eq!(world.turn_state(start.turn_id).await, "running");
    clock.advance(chrono::Duration::from_std(FORCE_AFTER).unwrap());
    world.wait_turn(start.turn_id, "interrupted").await;
    world.wait_blocked(thread, "error").await;

    let code: Option<String> = world
        .scalar(
            "SELECT error->>'code' FROM turns WHERE id = $1",
            start.turn_id,
        )
        .await;
    assert_eq!(code.as_deref(), Some("forced"));
    let message: String = world
        .scalar(
            "SELECT status FROM items WHERE turn_id = $1 AND kind = 'agent_message'",
            start.turn_id,
        )
        .await;
    assert_eq!(message, "cancelled", "what was half said is settled");
    let errors = world
        .count(
            "SELECT count(*) FROM items WHERE turn_id = $1 AND kind = 'error'",
            start.turn_id,
        )
        .await;
    assert_eq!(
        errors, 1,
        "the timeline says that the turn was ended without the harness"
    );
    assert_eq!(
        world.commands.issued_of("session.close"),
        0,
        "a forced controller end cannot close a session that may hold earlier retained commands"
    );
    world.machine.handler.release();
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Interrupted))
        .await;
    world.wait_state(thread, "idle").await;
    assert_eq!(world.turn_state(start.turn_id).await, "interrupted");
    assert!(
        world
            .scalar::<bool>(
                "SELECT native_end_confirmed FROM turns WHERE id=$1",
                start.turn_id
            )
            .await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_input_that_interrupts_stops_the_turn_and_goes_before_those_that_wait() {
    let world = World::new().await;
    let thread = world.thread("Cut in").await;
    let (start, _) = world.running_turn(thread, "Long job").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&ada, thread, queued("Later"))
        .await
        .unwrap();
    world
        .engine()
        .submit_input(&ada, thread, interrupting("Stop, do this instead"))
        .await
        .unwrap();

    world.wait_ran(CommandKind::TurnInterrupt, 1).await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Interrupted))
        .await;
    let cut_in = world.turn_start(thread, 1).await;
    assert_eq!(
        common::words_after_where(&cut_in.text),
        format!("{STOPPED_BY_ADA}\n\nStop, do this instead")
    );
    world.answer(thread, &cut_in, "m2", "Switched.").await;
    world
        .say(thread, turn_ended(cut_in.turn_id, TurnOutcome::Completed))
        .await;
    let later = world.turn_start(thread, 2).await;
    assert_eq!(common::words_after_where(&later.text), "Later");
    world
        .say(thread, turn_ended(later.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_input_sent_again_under_the_same_value_is_one_input_and_one_turn() {
    let world = World::new().await;
    let thread = world.thread("Twice").await;
    let ada = Caller::session(world.ada);
    let request = queued("Only once");
    let first = world
        .engine()
        .submit_input(&ada, thread, request.clone())
        .await
        .unwrap();
    let again = world
        .engine()
        .submit_input(&ada, thread, request.clone())
        .await
        .unwrap();
    assert!(first.created);
    assert!(!again.created, "the sender had sent it before");
    assert_eq!(first.input.id, again.input.id);

    world.run_turn(thread, 0, "Done.").await;
    let late = world
        .engine()
        .submit_input(&ada, thread, request)
        .await
        .unwrap();
    assert!(!late.created, "even after it was served");
    assert_eq!(late.input.id, first.input.id);
    world.wait_state(thread, "idle").await;
    meanwhile(Duration::from_millis(200)).await;
    assert_eq!(
        world
            .count("SELECT count(*) FROM turns WHERE thread_id = $1", thread)
            .await,
        1
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM thread_inputs WHERE thread_id = $1",
                thread
            )
            .await,
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inputs_sent_at_the_same_moment_each_become_exactly_one_turn_one_after_another() {
    const HOW_MANY: usize = 6;
    let world = World::new().await;
    let thread = world.thread("Crowd").await;
    let mut sends = Vec::new();
    for n in 0..HOW_MANY {
        let engine = world.engine();
        let caller = Caller::session(world.ada);
        sends.push(tokio::spawn(async move {
            engine
                .submit_input(&caller, thread, queued(&format!("Input {n}")))
                .await
                .unwrap()
        }));
    }
    let mut accepted = Vec::new();
    for send in sends {
        accepted.push(send.await.unwrap());
    }
    assert!(accepted.iter().all(|submitted| submitted.created));

    let mut texts = Vec::new();
    for n in 0..HOW_MANY {
        let start = world.run_turn(thread, n, &format!("Reply {n}")).await;
        texts.push(start.text);
    }
    world.wait_state(thread, "idle").await;
    let mut sorted = texts.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        HOW_MANY,
        "no input was served twice: {texts:?}"
    );
    assert_eq!(
        world
            .count("SELECT count(*) FROM turns WHERE thread_id = $1", thread)
            .await,
        HOW_MANY as i64
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM thread_inputs WHERE thread_id = $1 AND state = 'delivered'",
                thread
            )
            .await,
        HOW_MANY as i64
    );
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(
        world.turn_starts(thread).len(),
        HOW_MANY,
        "and none was started again"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_who_may_not_interact_cannot_send_an_input() {
    let world = World::new().await;
    let thread = world.thread("Closed door").await;
    let stranger = world.person("eve").await;
    let refused = world
        .engine()
        .submit_input(&Caller::session(stranger), thread, queued("Let me in"))
        .await;
    assert!(matches!(refused, Err(InputError::Denied(_))), "{refused:?}");
    meanwhile(Duration::from_millis(150)).await;
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM thread_inputs WHERE thread_id = $1",
                thread
            )
            .await,
        0
    );
    eventually("the thread is idle", || async {
        world.state(thread).await.0 == "idle"
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_who_may_not_use_the_provider_cannot_send_any_kind_of_input() {
    let world = World::new().await;
    let thread = world.thread("No provider").await;
    let bob = world.person("bob").await;
    world.give(bob, "interactor").await;
    world.access.deny(bob);
    // As shipped, a writer without the model's use is taken and held for somebody who may run it; where the
    // place refuses such writers, no kind of input of his is taken.
    world
        .set(
            world.topic,
            orch_settings_contract::registry::keys::THREAD_INPUTS_WITHOUT_USE,
            serde_json::json!("refused"),
        )
        .await;
    for request in [
        queued("Queue"),
        steering("Steer"),
        interrupting("Interrupt"),
    ] {
        let refused = world
            .engine()
            .submit_input(&Caller::session(bob), thread, request)
            .await;
        assert!(matches!(refused, Err(InputError::Denied(_))), "{refused:?}");
    }
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM thread_inputs WHERE thread_id = $1",
                thread
            )
            .await,
        0
    );
    // What he lacks is told whole, and the others' inputs are not in his way.
    world.access.allow(bob);
    let accepted = world
        .engine()
        .submit_input(&Caller::session(bob), thread, queued("Now"))
        .await
        .unwrap();
    assert!(accepted.created);
}

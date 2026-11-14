//! A person's Stop is taken the moment it is asked, whatever else holds the thread, and the people who work on one
//! thread at the same moment leave it in one state.
use super::*;
use orch_api_contract::threads::ContentPart;
use orch_db::queries::threads;

/// How long a Stop may take to be answered while something else holds the thread's row. The row is held far
/// longer than this in every case below.
const ANSWERED_WITHIN: Duration = Duration::from_secs(3);

/// Holds the row of `thread`, as any other change of the thread does, until the returned transaction ends.
async fn hold_row(world: &World, thread: orch_types::Id) -> orch_db::Tx {
    let mut holder = world.pool.begin().await.unwrap();
    threads::lock(&mut holder, thread).await.unwrap();
    holder
}

async fn open_stops(world: &World, thread: orch_types::Id) -> i64 {
    world
        .count(
            "SELECT count(*) FROM thread_stops WHERE thread_id = $1",
            thread,
        )
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_is_answered_while_another_change_holds_the_threads_row_and_is_carried_out_when_it_lets_go()
 {
    let world = World::new().await;
    let thread = world.thread("Stop behind a held row").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    let ada = Caller::session(world.ada);
    let holder = hold_row(&world, thread).await;
    let engine = world.engine();
    let answered = tokio::time::timeout(
        ANSWERED_WITHIN,
        engine.stop_thread(&ada, thread, Some(start.turn_id)),
    )
    .await;
    let stopped = answered
        .expect("a Stop is answered while somebody else holds the thread's row")
        .unwrap();
    assert!(
        !stopped.applied,
        "the row is held, so the thread has not carried the Stop out yet"
    );
    let turn = stopped.turn.expect("the answer is the turn it names");
    assert!(
        turn.interrupted_by.is_some(),
        "the answer says the turn is being stopped, and by whom"
    );
    assert_eq!(world.turn_state(start.turn_id).await, "running");
    assert_eq!(open_stops(&world, thread).await, 1);
    holder.commit().await.unwrap();
    eventually("the harness is asked to stop", || async {
        world.commands.issued_of("turn.interrupt") == 1
    })
    .await;
    eventually("the Stop is carried out and forgotten", || async {
        open_stops(&world, thread).await == 0
    })
    .await;
    let marked: Option<chrono::DateTime<chrono::Utc>> = world
        .scalar(
            "SELECT interrupted_at FROM turns WHERE id = $1",
            start.turn_id,
        )
        .await;
    assert!(
        marked.is_some_and(|at| at <= stopped.since),
        "the turn says when the Stop was asked, not when the row was free"
    );
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Interrupted))
        .await;
    world.wait_state(thread, "idle").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_that_takes_the_row_after_a_stop_that_waits_for_it_joins_the_stops_hold() {
    let world = World::new().await;
    let thread = world.thread("Send after a waiting stop").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    let ada = Caller::session(world.ada);
    let holder = hold_row(&world, thread).await;
    let engine = world.engine();
    tokio::time::timeout(
        ANSWERED_WITHIN,
        engine.stop_thread(&ada, thread, Some(start.turn_id)),
    )
    .await
    .expect("answered")
    .unwrap();
    let sending = {
        let engine = world.engine();
        let ada = ada.clone();
        tokio::spawn(async move {
            engine
                .submit_input(&ada, thread, steering("After the Stop"))
                .await
        })
    };
    meanwhile(Duration::from_millis(300)).await;
    holder.commit().await.unwrap();
    let sent = sending.await.unwrap().unwrap();
    let sync = world.sync(thread).await;
    let row = sync
        .inputs
        .iter()
        .find(|input| input.id == sent.input.id)
        .unwrap();
    assert!(
        row.queue_held,
        "a message sent after the Stop waits with what the Stop holds"
    );
    assert_eq!(row.state, InputState::Pending);
    assert!(
        row.turn_id.is_none(),
        "and is not steered into the turn that is stopping"
    );
    assert_eq!(world.commands.issued_of("turn.steer"), 0);
    assert_eq!(open_stops(&world, thread).await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_people_sending_while_a_third_stops_leave_one_held_queue_in_the_order_it_was_taken() {
    let world = World::new().await;
    let thread = world.thread("Two senders and a stop").await;
    let bob = world.person("bob").await;
    world.give(bob, "interactor").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    let ada = Caller::session(world.ada);
    let bob = Caller::session(bob);
    let engine = world.engine();
    let ada_sends = async {
        let mut ids = Vec::new();
        for words in ["a1", "a2", "a3"] {
            ids.push(
                engine
                    .submit_input(&ada, thread, queued(words))
                    .await
                    .unwrap()
                    .input
                    .id,
            );
        }
        ids
    };
    let bob_sends = async {
        let mut ids = Vec::new();
        for words in ["b1", "b2", "b3"] {
            ids.push(
                engine
                    .submit_input(&bob, thread, queued(words))
                    .await
                    .unwrap()
                    .input
                    .id,
            );
        }
        ids
    };
    let stops = engine.stop_thread(&ada, thread, Some(start.turn_id));
    let (ada_ids, bob_ids, stopped) = tokio::join!(ada_sends, bob_sends, stops);
    stopped.unwrap();
    let sync = world.sync(thread).await;
    assert!(sync.thread.queue.held);
    assert_eq!(sync.thread.queue.held_inputs, 6);
    let mut waiting: Vec<_> = sync
        .inputs
        .iter()
        .filter(|input| input.state == InputState::Pending)
        .collect();
    waiting.sort_by_key(|input| input.position.parse::<i64>().unwrap());
    assert_eq!(waiting.len(), 6);
    assert!(waiting.iter().all(|input| input.queue_held));
    let positions: Vec<i64> = waiting
        .iter()
        .map(|input| input.position.parse().unwrap())
        .collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "every message has a place of its own: {positions:?}"
    );
    for own in [&ada_ids, &bob_ids] {
        let places: Vec<i64> = own
            .iter()
            .map(|id| {
                waiting
                    .iter()
                    .find(|input| input.id == *id)
                    .unwrap()
                    .position
                    .parse()
                    .unwrap()
            })
            .collect();
        assert!(
            places.windows(2).all(|pair| pair[0] < pair[1]),
            "each person's messages keep the order they sent them in: {places:?}"
        );
    }
    assert_eq!(world.turn_starts(thread).len(), 1);
    assert_eq!(world.commands.issued_of("turn.interrupt"), 1);
    let again = world.sync(thread).await;
    assert_eq!(
        again.inputs.len(),
        sync.inputs.len(),
        "a second reading says the same"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn editing_a_waiting_message_while_another_person_sends_keeps_both() {
    let world = World::new().await;
    let thread = world.thread("Edit and send").await;
    let bob = world.person("bob").await;
    world.give(bob, "interactor").await;
    let (_start, _) = world.running_turn(thread, "Begin").await;
    let ada = Caller::session(world.ada);
    let bob = Caller::session(bob);
    let waiting = world
        .engine()
        .submit_input(&ada, thread, queued("First draft"))
        .await
        .unwrap();
    let engine = world.engine();
    let (edited, sent) = tokio::join!(
        engine.edit_input(
            &ada,
            thread,
            waiting.input.id,
            Some(vec![ContentPart::Text {
                text: "Second draft".to_owned()
            }]),
            None,
        ),
        engine.submit_input(&bob, thread, queued("From bob")),
    );
    edited.unwrap();
    let sent = sent.unwrap();
    let sync = world.sync(thread).await;
    let pending: Vec<_> = sync
        .inputs
        .iter()
        .filter(|input| input.state == InputState::Pending)
        .collect();
    assert_eq!(pending.len(), 2);
    let edited = pending
        .iter()
        .find(|input| input.id == waiting.input.id)
        .unwrap();
    assert!(
        format!("{:?}", edited.content).contains("Second draft"),
        "the edit is kept: {:?}",
        edited.content
    );
    assert!(pending.iter().any(|input| input.id == sent.input.id));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_of_a_turn_that_is_over_is_answered_as_it_ended_without_waiting_for_the_row() {
    let world = World::new().await;
    let thread = world.thread("Stop after the end").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&ada, thread, queued("First"))
        .await
        .unwrap();
    let first = world.run_turn(thread, 0, "Done.").await;
    world.wait_state(thread, "idle").await;
    let holder = hold_row(&world, thread).await;
    let stopped = tokio::time::timeout(
        ANSWERED_WITHIN,
        world
            .engine()
            .stop_thread(&ada, thread, Some(first.turn_id)),
    )
    .await
    .expect("answered while the row is held")
    .unwrap();
    assert_eq!(
        stopped.turn.expect("names the turn").state,
        TurnState::Completed,
        "the turn is shown as it ended"
    );
    holder.commit().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sending_to_a_thread_does_not_wait_for_its_previous_turns_notification_fanout() {
    let world = World::new().await;
    let thread = world.thread("Send during completion notices").await;
    let (start, _) = world.running_turn(thread, "First").await;
    let mut fanout = world.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE notifications IN SHARE MODE")
        .execute(&mut *fanout)
        .await
        .unwrap();
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    eventually("completion fanout is waiting for its table", || async {
        world.scalar::<bool>("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE relation='notifications'::regclass AND NOT granted) AND EXISTS(SELECT 1 FROM turns WHERE id=$1 AND end_work IS NOT NULL)", start.turn_id).await
    }).await;
    let submitted = tokio::time::timeout(
        ANSWERED_WITHIN,
        world
            .engine()
            .submit_input(&Caller::session(world.ada), thread, queued("Next")),
    )
    .await
    .expect("a send does not wait behind notification recipients")
    .unwrap();
    assert_eq!(submitted.input.state, InputState::Pending);
    fanout.commit().await.unwrap();
    let next = world.turn_start(thread, 1).await;
    assert!(next.text.contains("Next"));
    world.restart_api().await;
    let notices: i64 = world
        .scalar(
            "SELECT count(*) FROM notifications WHERE thread_id=$1 AND kind='turn.completed'",
            thread,
        )
        .await;
    assert_eq!(
        notices, 1,
        "the durable effect receipt never repeats fanout"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stopped_thread_leaves_the_shared_process_start_and_its_next_turn_can_take_its_place() {
    let world = World::new().await;
    let thread = world.thread("Withdraw from shared start").await;
    let caller = Caller::session(world.ada);
    world.machine.handler.hold(CommandKind::AgentStart);
    world
        .engine()
        .submit_input(&caller, thread, queued("Continue after Stop"))
        .await
        .unwrap();
    world.wait_began(CommandKind::AgentStart, 1).await;
    let old: orch_types::Id = world
        .scalar(
            "SELECT current_turn_id FROM threads WHERE node_id=$1",
            thread,
        )
        .await;
    world
        .engine()
        .stop_thread(&caller, thread, Some(old))
        .await
        .unwrap();
    world.wait_turn(old, "interrupted").await;
    let queue = world.sync(thread).await.thread.queue;
    world
        .engine()
        .resume_queue(&caller, thread, queue.revision)
        .await
        .unwrap();
    tokio::time::timeout(ANSWERED_WITHIN, eventually("the new turn reaches its own harness wait", || async {
        sqlx::query_scalar::<_, bool>("SELECT current_turn_id IS DISTINCT FROM $2 AND current_turn_id IS NOT NULL AND starting_step='harness' FROM threads WHERE node_id=$1")
            .bind(thread).bind(old).fetch_one(world.pool.pool()).await.unwrap()
    })).await.expect("the next turn does not wait behind the stopped thread");
    assert_eq!(
        world.commands.issued_of("agent.start"),
        1,
        "the shared process continues its original start"
    );
    world.machine.handler.release();
    let next = world.turn_start(thread, 0).await;
    assert_ne!(next.turn_id, old);
    assert!(next.text.contains("Continue after Stop"));
    assert_eq!(world.ran(CommandKind::AgentStart), 1);
}

//! What a harness says is applied exactly once, in order, to the thread it is about and to no other.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{
    World, eventually, meanwhile, message_began, message_done, message_grew, queued, said,
    turn_began, turn_ended,
};
use orch_authz::Caller;
use orch_link::streams::DeliveredEvent;
use orch_link_contract::events::{
    DeltaField, EmittedItemKind, Event, ItemCompleted, ItemDelta, ItemStarted, SessionOpened,
    SessionTitle, TurnOutcome, TurnPhase, TurnPhaseKind, UsageUpdated,
};
use orch_link_contract::streams::Stream;
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::time::Duration;

fn delivered(world: &World, seq: i64, event: &Event) -> DeliveredEvent {
    DeliveredEvent {
        seq,
        at: Time::new(world.clock.now()),
        kind: event.kind().as_str().to_owned(),
        body: event.body().unwrap(),
    }
}

/// Applies events as the link would hand them over, in a transaction of their own.
async fn apply(world: &World, thread: Id, segment: Id, events: &[DeliveredEvent]) {
    world.open_session_for_reports(thread, segment).await;
    let mut tx = world.pool.begin().await.unwrap();
    world
        .engine()
        .apply_stream_events(
            &mut tx,
            world.machine_id,
            &Stream::Session {
                thread_id: thread,
                segment_id: segment,
            },
            events,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

/// Everything the database holds of a thread, as one document.
async fn picture(world: &World, thread: Id) -> Json {
    let (whole,): (Json,) = sqlx::query_as(
        "SELECT jsonb_build_object(
            'thread', (SELECT to_jsonb(t) FROM threads t WHERE t.node_id = $1),
            'items', (SELECT coalesce(jsonb_agg(to_jsonb(i) ORDER BY i.seq_created, i.id), '[]'::jsonb)
                        FROM items i WHERE i.thread_id = $1),
            'turns', (SELECT coalesce(jsonb_agg(to_jsonb(u) ORDER BY u.ordinal), '[]'::jsonb)
                        FROM turns u WHERE u.thread_id = $1),
            'inputs', (SELECT coalesce(jsonb_agg(to_jsonb(n) ORDER BY n.position), '[]'::jsonb)
                         FROM thread_inputs n WHERE n.thread_id = $1),
            'segments', (SELECT coalesce(jsonb_agg(to_jsonb(s) ORDER BY s.ordinal), '[]'::jsonb)
                           FROM segments s WHERE s.thread_id = $1),
            'model_inputs', (SELECT coalesce(jsonb_agg(to_jsonb(m) ORDER BY m.seq_created, m.id), '[]'::jsonb)
                              FROM model_inputs m WHERE m.node_id = $1),
            'approvals', (SELECT coalesce(jsonb_agg(to_jsonb(a) ORDER BY a.id), '[]'::jsonb)
                            FROM approvals a WHERE a.thread_id = $1))",
    )
    .bind(thread)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    whole
}

/// Command receipt precedes the engine's durable record of what reached the model.
async fn model_inputs_settled(world: &World, thread: Id) {
    eventually("the model input command results are recorded", || async {
        world
            .count(
                "SELECT count(*) FROM model_inputs WHERE node_id=$1 AND state='prepared'",
                thread,
            )
            .await
            == 0
    })
    .await;
}

fn usage(turn: Id) -> Event {
    let figures = json!({
        "input_tokens": 100, "cached_input_tokens": 10, "output_tokens": 20,
        "reasoning_tokens": 5, "total_tokens": 120
    });
    Event::UsageUpdated(UsageUpdated {
        turn_id: turn,
        turn: serde_json::from_value(figures.clone()).unwrap(),
        thread: serde_json::from_value(figures).unwrap(),
        context_window: Some(200_000),
        context_used: Some(5_000),
        report: None,
    })
}

/// A turn's worth of everything a session says.
fn script(world: &World, segment: Id, turn: Id) -> Vec<DeliveredEvent> {
    let at = world.clock.now();
    let events = vec![
        Event::SessionOpened(SessionOpened {
            native_session_ref: format!("native-{segment}"),
            resumed: false,
        }),
        turn_began(turn),
        Event::SessionTitle(SessionTitle {
            title: "Fix the login page".to_owned(),
        }),
        message_began(turn, "m1"),
        message_grew("m1", 0, "I will "),
        message_grew("m1", 7, "look at it."),
        Event::ItemStarted(ItemStarted {
            turn_id: turn,
            native_ref: "r1".to_owned(),
            stream: "main".to_owned(),
            parent_native_ref: None,
            kind: EmittedItemKind::Reasoning,
            payload: orch_link_contract::foreign::ItemPayload::Reasoning(
                orch_agent_contract::events::ReasoningPayload {
                    summary: String::new(),
                    text_len: 0,
                    hidden: false,
                },
            ),
        }),
        Event::ItemDelta(ItemDelta {
            native_ref: "r1".to_owned(),
            field: DeltaField::Summary,
            offset: 0,
            len: 5,
            data: "Think".to_owned(),
        }),
        Event::TurnPhase(TurnPhase {
            turn_id: turn,
            kind: TurnPhaseKind::Compacting,
            at: Time::new(at),
        }),
        Event::TurnPhase(TurnPhase {
            turn_id: turn,
            kind: TurnPhaseKind::Working,
            at: Time::new(at + chrono::Duration::seconds(2)),
        }),
        usage(turn),
        message_done("m1", "I will look at it."),
        Event::ItemCompleted(ItemCompleted {
            native_ref: "r1".to_owned(),
            status: orch_agent_contract::events::ItemStatus::Completed,
            payload: orch_link_contract::foreign::ItemPayload::Reasoning(
                orch_agent_contract::events::ReasoningPayload {
                    summary: "Think".to_owned(),
                    text_len: 5,
                    hidden: false,
                },
            ),
            output_blob: None,
        }),
        turn_ended(turn, TurnOutcome::Completed),
    ];
    events
        .iter()
        .zip(1_i64..)
        .map(|(event, seq)| delivered(world, seq, event))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_events_delivered_twice_change_nothing_the_second_time() {
    let world = World::new().await;
    let thread = world.thread("Twice delivered").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&ada, thread, queued("Please do it"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    let segment = world.segment_of(thread).await;
    let events = script(&world, segment, start.turn_id);

    apply(&world, thread, segment, &events).await;
    let forwarded = world.live.deltas().len();
    let sync = world.sync(thread).await;
    assert_eq!(
        said(&sync),
        ["user: Please do it", "agent: I will look at it."]
    );
    assert_eq!(
        sync.turns[0].state,
        orch_api_contract::threads::TurnState::Completed
    );
    assert!(sync.thread.usage.is_some(), "the thread's use is told");
    assert_eq!(
        forwarded, 2,
        "each stretch of text was forwarded once, the two that continue each other in one delivery as one"
    );
    world.wait_state(thread, "idle").await;
    // What the end of the turn writes after it (its use counted, who is told) is part of the picture.
    world.wait_followed_up(thread).await;
    model_inputs_settled(&world, thread).await;
    let once = picture(&world, thread).await;

    // The whole of it again, and then the stretches a reconnecting link would resend.
    for (name, window) in [
        ("all of it", &events[..]),
        ("the second half", &events[events.len() / 2..]),
        ("the first half", &events[..events.len() / 2]),
        ("the middle", &events[3..9]),
        ("the last", &events[events.len() - 1..]),
        ("the first", &events[..1]),
    ] {
        apply(&world, thread, segment, window).await;
        assert_eq!(
            picture(&world, thread).await,
            once,
            "delivered again: {name}"
        );
    }
    assert_eq!(
        world.live.deltas().len(),
        forwarded,
        "nothing already forwarded is forwarded again"
    );

    // A second turn begins; the first turn's words, delivered late, leave it as it is.
    world
        .engine()
        .submit_input(&ada, thread, queued("And another thing"))
        .await
        .unwrap();
    let second = world.turn_start(thread, 1).await;
    model_inputs_settled(&world, thread).await;
    let before = picture(&world, thread).await;
    apply(&world, thread, segment, &events).await;
    assert_eq!(
        picture(&world, thread).await,
        before,
        "words of a turn that is over leave the next alone"
    );
    assert_eq!(world.turn_state(second.turn_id).await, "starting");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stretch_of_text_that_skips_ahead_is_left_for_the_completed_item() {
    let world = World::new().await;
    let thread = world.thread("Gap").await;
    let (start, _) = world.running_turn(thread, "Say something long").await;
    world.say(thread, message_began(start.turn_id, "m2")).await;
    world.say(thread, message_grew("m2", 0, "abc")).await;
    // A stretch that skips ahead, and one that was already applied.
    world.say(thread, message_grew("m2", 10, "xyz")).await;
    world.say(thread, message_grew("m2", 0, "abc")).await;
    common::eventually("the first stretch is applied", || async {
        world.sync(thread).await.items.iter().any(|item| {
            matches!(
                &item.payload,
                orch_api_contract::threads::ItemPayload::AgentMessage(m) if m.text == "abc"
            )
        })
    })
    .await;
    meanwhile(Duration::from_millis(200)).await;
    let texts: Vec<_> = world
        .live
        .deltas()
        .iter()
        .filter(|d| d.data != "Working")
        .map(|d| d.data.clone())
        .collect();
    assert_eq!(
        texts,
        ["abc"],
        "only text that continues what is held is forwarded"
    );

    world
        .say(thread, message_done("m2", "abcdefghijklxyz"))
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    let said = said(&world.sync(thread).await);
    assert_eq!(
        said.last().map(String::as_str),
        Some("agent: abcdefghijklxyz"),
        "the completed item has the whole"
    );
}

/// The text of the thread's last message of the agent, as the thread reads back.
async fn message_text(world: &World, thread: Id) -> Option<String> {
    world
        .sync(thread)
        .await
        .items
        .iter()
        .rev()
        .find_map(|item| match &item.payload {
            orch_api_contract::threads::ItemPayload::AgentMessage(message) => {
                Some(message.text.clone())
            }
            _ => None,
        })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn growing_text_is_held_and_written_before_what_ends_its_turn() {
    // An interval no test reaches: only what the session says next writes what is held.
    let world = World::tuned(|config| config.stream_flush = Duration::from_secs(3_600)).await;
    let thread = world.thread("Held").await;
    let (start, _) = world.running_turn(thread, "Begin, then stop").await;
    world.say(thread, message_began(start.turn_id, "m4")).await;
    world.say(thread, message_grew("m4", 0, "Half of ")).await;
    world.say(thread, message_grew("m4", 8, "an answer")).await;
    eventually("both stretches reach the live readers", || async {
        world
            .live
            .deltas()
            .iter()
            .filter(|delta| delta.data == "Half of " || delta.data == "an answer")
            .count()
            == 2
    })
    .await;
    meanwhile(Duration::from_millis(200)).await;
    assert_eq!(
        message_text(&world, thread).await.as_deref(),
        Some(""),
        "the pieces are held, not written to the item"
    );

    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Interrupted))
        .await;
    eventually("the end of the turn wrote what was held", || async {
        message_text(&world, thread).await.as_deref() == Some("Half of an answer")
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn events_of_two_threads_interleaved_stay_apart() {
    let world = World::new().await;
    let (first, second) = (world.thread("First").await, world.thread("Second").await);
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&ada, first, queued("Question one"))
        .await
        .unwrap();
    world
        .engine()
        .submit_input(&ada, second, queued("Question two"))
        .await
        .unwrap();
    let (one, two) = (
        world.turn_start(first, 0).await,
        world.turn_start(second, 0).await,
    );
    let (segment_one, segment_two) = (
        world.segment_of(first).await,
        world.segment_of(second).await,
    );
    assert_ne!(segment_one, segment_two);

    // Both sessions use the same names for their items; the words are their own.
    let speak = |thread: Id, turn: Id, words: Vec<&'static str>| {
        let machine = &world;
        async move {
            machine.say(thread, turn_began(turn)).await;
            machine.say(thread, message_began(turn, "m1")).await;
            let mut before = 0_i64;
            let mut whole = String::new();
            for part in words {
                machine.say(thread, message_grew("m1", before, part)).await;
                before += i64::try_from(part.chars().count()).unwrap();
                whole.push_str(part);
                tokio::task::yield_now().await;
            }
            machine.say(thread, message_done("m1", &whole)).await;
            machine
                .say(thread, turn_ended(turn, TurnOutcome::Completed))
                .await;
        }
    };
    tokio::join!(
        speak(
            first,
            one.turn_id,
            vec!["Answer ", "to ", "the ", "first ", "question."]
        ),
        speak(
            second,
            two.turn_id,
            vec!["The ", "second ", "answer ", "is ", "different."]
        )
    );
    world.wait_state(first, "idle").await;
    world.wait_state(second, "idle").await;
    world.wait_followed_up(first).await;
    world.wait_followed_up(second).await;
    model_inputs_settled(&world, first).await;
    model_inputs_settled(&world, second).await;

    let (sync_one, sync_two) = (world.sync(first).await, world.sync(second).await);
    assert_eq!(
        said(&sync_one),
        ["user: Question one", "agent: Answer to the first question."]
    );
    assert_eq!(
        said(&sync_two),
        [
            "user: Question two",
            "agent: The second answer is different."
        ]
    );
    for (thread, sync) in [(first, &sync_one), (second, &sync_two)] {
        assert!(
            sync.items.iter().all(|item| item.thread_id == thread),
            "nothing of the other thread"
        );
        // Each thread's sequence is its own and has no gap in what readers see.
        let next: i64 = world
            .scalar("SELECT next_seq FROM threads WHERE node_id = $1", thread)
            .await;
        assert_eq!(sync.seq, next - 1);
        let mut seen: Vec<i64> = sync
            .items
            .iter()
            .flat_map(|item| [item.seq_created, item.seq_updated])
            .collect();
        seen.extend(sync.turns.iter().map(|turn| turn.seq_updated));
        assert!(
            seen.iter().all(|seq| (1..next).contains(seq)),
            "{seen:?} within 1..{next}"
        );
    }
}

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
            retry_attempt: None,
            at: Time::new(at),
        }),
        Event::TurnPhase(TurnPhase {
            turn_id: turn,
            kind: TurnPhaseKind::Working,
            retry_attempt: None,
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
    let observed = std::sync::Mutex::new(None::<Json>);
    eventually("both stretches reach the live readers", || async {
        let item: Option<Id> =
            sqlx::query_scalar("SELECT id FROM items WHERE thread_id=$1 AND native_ref='m4'")
                .bind(thread)
                .fetch_optional(world.pool.pool())
                .await
                .unwrap();
        let deltas = world.live.deltas();
        let snapshot = json!(deltas.iter().map(|delta| {
            json!({"thread_id":delta.thread_id,"item_id":delta.item_id,"turn_id":delta.turn_id,
                "stream":delta.stream,"field":delta.field,"offset":delta.offset,
                "len":delta.len,"data":delta.data})
        }).collect::<Vec<_>>());
        let mut previous = observed.lock().unwrap();
        if previous.as_ref() != Some(&snapshot) {
            eprintln!(
                "held-text-live thread={thread} turn={} deltas={snapshot}",
                start.turn_id
            );
            *previous = Some(snapshot);
        }
        drop(previous);
        deltas
            .iter()
            .filter(|delta| {
                delta.thread_id == thread
                    && Some(delta.item_id) == item
                    && delta.field == DeltaField::Text
            })
            .map(|delta| delta.data.as_str())
            .collect::<String>()
            == "Half of an answer"
    })
    .await;
    meanwhile(Duration::from_millis(200)).await;
    assert_eq!(
        message_text(&world, thread).await.as_deref(),
        Some(""),
        "the pieces are held, not written to the item"
    );
    let item: Id = world
        .scalar(
            "SELECT id FROM items WHERE thread_id=$1 AND native_ref='m4'",
            thread,
        )
        .await;
    let mut offset = 0;
    let mut streamed = String::new();
    for delta in world
        .live
        .deltas()
        .into_iter()
        .filter(|delta| delta.item_id == item)
    {
        assert_eq!(delta.thread_id, thread);
        assert_eq!(delta.item_id, item);
        assert_eq!(delta.turn_id, Some(start.turn_id));
        assert_eq!(delta.stream, "main");
        assert_eq!(delta.field, DeltaField::Text);
        assert_eq!(delta.offset, offset, "live text has no gaps or duplicates");
        assert_eq!(
            delta.len,
            i64::try_from(delta.data.chars().count()).unwrap()
        );
        offset += delta.len;
        streamed.push_str(&delta.data);
    }
    assert_eq!(streamed, "Half of an answer");
    assert_eq!(offset, i64::try_from(streamed.chars().count()).unwrap());

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_native_settings_observation_stays_outside_an_already_reserved_turn() {
    let world = World::new().await;
    let thread = world.thread("Native settings").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let before: Json = world
        .scalar(
            "SELECT jsonb_build_object('model',model,'params',params) FROM turns WHERE id=$1",
            start.turn_id,
        )
        .await;
    let event=Event::decode("item.completed",json!({"native_ref":"native-settings-idle","status":"completed","payload":{"marker":"harness_changed","data":{"program":"Codex","change":"model","previous":{"state":"told","value":"chosen"},"value":"native-future","expected":{"state":"told","value":"chosen"},"changed":true,"diverged":true,"next_turn":true,"turn_id":null,"reported_at":world.clock.time()}}})).unwrap();
    world.say(thread, event.clone()).await;
    world.say(thread, event).await;
    eventually("one source-owned native settings marker",||async {world.scalar::<bool>("SELECT count(*)=1 FROM items WHERE thread_id=$1 AND native_ref='native-settings-idle'",thread).await}).await;
    let marker_turn: Option<Id> = world
        .scalar(
            "SELECT turn_id FROM items WHERE thread_id=$1 AND native_ref='native-settings-idle'",
            thread,
        )
        .await;
    assert!(
        marker_turn.is_none(),
        "a thread-level observation must not invent an originating turn"
    );
    let after: Json = world
        .scalar(
            "SELECT jsonb_build_object('model',model,'params',params) FROM turns WHERE id=$1",
            start.turn_id,
        )
        .await;
    assert_eq!(before, after);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anonymous_native_failures_keep_source_event_identity_across_transport_replay() {
    let world = World::new().await;
    let thread = world.thread("Anonymous failure observations").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    let segment = world.segment_of(thread).await;
    let running = orch_agent_contract::requests::UsageRequest {
        id: "native-session:1".into(),
        asked: Some(world.clock.time()),
        ..Default::default()
    };
    let mut first = running.clone();
    first.id = "native-session:native-turn:failure:1".into();
    first.previous_ref = Some(running.id.clone());
    first.upstream_status = Some(429);
    first.finish(
        "rate_limited",
        Some("Same native words"),
        Some("Same vendor words"),
    );
    let mut second = first.clone();
    second.id = "native-session:native-turn:failure:2".into();
    second.previous_ref = None;
    let reported = |request| {
        Event::ModelRequest(orch_link_contract::events::ModelRequest {
            turn_id: start.turn_id,
            request,
        })
    };
    let first_event = reported(first.clone());
    let events = [
        delivered(
            &world,
            1,
            &Event::SessionOpened(SessionOpened {
                native_session_ref: "native-session".into(),
                resumed: false,
            }),
        ),
        delivered(&world, 2, &turn_began(start.turn_id)),
        delivered(&world, 3, &reported(running)),
        delivered(&world, 4, &first_event),
        delivered(&world, 5, &reported(second)),
    ];
    apply(&world, thread, segment, &events).await;
    let rows = world.ledger_of(start.turn_id).await;
    assert_eq!(
        rows.len(),
        2,
        "an exact provisional observation is replaced by its failure report: {rows:?}"
    );
    assert_ne!(rows[0].id, rows[1].id);
    for row in &rows {
        assert_eq!(row.total_tokens, 0);
        assert_eq!(row.upstream_status, Some(429));
        assert!(row.provider_request_id.is_none());
        assert_eq!(row.outcome.as_deref(), Some("rate_limited"));
    }
    assert_eq!(
        world
            .scalar::<i64>(
                "SELECT count(*) FROM usage_running WHERE turn_id=$1",
                start.turn_id
            )
            .await,
        0
    );
    apply(&world, thread, segment, &events).await;
    apply(
        &world,
        thread,
        segment,
        &[delivered(&world, 6, &first_event)],
    )
    .await;
    let replayed = world.ledger_of(start.turn_id).await;
    assert_eq!(
        replayed.len(),
        2,
        "the same reported failure event can be delivered again with its retained reference"
    );
    assert_eq!(rows, replayed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_marker_only_compaction_resends_context_once_without_borrowing_a_childs_context() {
    let world = World::new().await;
    let thread = world.thread("Marker-only compaction").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    let segment = world.segment_of(thread).await;
    let marker = |native_ref: &str| {
        Event::decode("item.completed",json!({"native_ref":native_ref,"status":"completed","payload":{"marker":"compacted","data":{"duration_ms":null}}})).unwrap()
    };
    let began = |native_ref: &str, stream: &str| {
        Event::decode("item.started",json!({"native_ref":native_ref,"turn_id":start.turn_id,"stream":stream,"kind":"marker","payload":{"marker":"compacted","data":{"duration_ms":null}}})).unwrap()
    };
    let reset = || async {
        sqlx::query("UPDATE segments SET context_sent='{\"baseline\":true}' WHERE id=$1")
            .bind(segment)
            .execute(world.pool.pool())
            .await
            .unwrap();
    };
    let sent = || async {
        world
            .scalar::<Json>("SELECT context_sent FROM segments WHERE id=$1", segment)
            .await
    };
    reset().await;
    apply(
        &world,
        thread,
        segment,
        &[
            delivered(
                &world,
                1,
                &Event::SessionOpened(SessionOpened {
                    native_session_ref: "native-compaction".into(),
                    resumed: false,
                }),
            ),
            delivered(&world, 2, &turn_began(start.turn_id)),
            delivered(&world, 3, &marker("only-end")),
        ],
    )
    .await;
    assert_eq!(sent().await, json!({"resend":true}), "no phase was needed");
    reset().await;
    apply(
        &world,
        thread,
        segment,
        &[delivered(&world, 4, &marker("only-end"))],
    )
    .await;
    assert_eq!(
        sent().await,
        json!({"baseline":true}),
        "replay does not re-arm a consumed resend"
    );
    apply(
        &world,
        thread,
        segment,
        &[
            delivered(&world, 5, &began("child-compaction", "sub:child")),
            delivered(&world, 6, &marker("child-compaction")),
        ],
    )
    .await;
    assert_eq!(
        sent().await,
        json!({"baseline":true}),
        "a child did not compact the main conversation"
    );
    apply(
        &world,
        thread,
        segment,
        &[
            delivered(&world, 7, &began("observed-compaction", "main")),
            delivered(&world, 8, &marker("observed-compaction")),
        ],
    )
    .await;
    assert_eq!(
        sent().await,
        json!({"resend":true}),
        "a started marker uses the same supplier"
    );
    reset().await;
    let data = json!({"program":"Codex","change":"context_compaction","subject":null,
        "previous":{"state":"unsupported"},"value":{"duration_ms":null},
        "expected":{"state":"unsupported"},"changed":true,"diverged":false,
        "next_turn":false,"turn_id":null,"reported_at":world.clock.time()});
    let report = Event::decode(
        "item.completed",
        json!({"native_ref":"idle-native-compaction","status":"completed",
        "payload":{"marker":"harness_changed","data":data}}),
    )
    .unwrap();
    apply(&world, thread, segment, &[delivered(&world, 9, &report)]).await;
    assert_eq!(
        sent().await,
        json!({"resend":true}),
        "an explicit unassigned native compaction uses the same supplier"
    );
    reset().await;
    apply(&world, thread, segment, &[delivered(&world, 10, &report)]).await;
    assert_eq!(
        sent().await,
        json!({"baseline":true}),
        "the same observed source does not re-arm a consumed resend"
    );
    let mut child_data = data;
    child_data["turn_id"] = json!(start.turn_id);
    let payload = json!({"marker":"harness_changed","data":child_data});
    let began = Event::decode(
        "item.started",
        json!({"native_ref":"native-child-compaction","turn_id":start.turn_id,
        "stream":"sub:child","kind":"marker","payload":payload}),
    )
    .unwrap();
    let ended = Event::decode(
        "item.completed",
        json!({"native_ref":"native-child-compaction","status":"completed","payload":payload}),
    )
    .unwrap();
    apply(
        &world,
        thread,
        segment,
        &[delivered(&world, 11, &began), delivered(&world, 12, &ended)],
    )
    .await;
    assert_eq!(
        sent().await,
        json!({"baseline":true}),
        "the new native marker preserves its child's separate context"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_runtime_changes_audit_their_source_once_without_copying_native_bodies() {
    let world = World::new().await;
    let thread = world.thread("Runtime audit").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    let segment = world.segment_of(thread).await;
    let changes = [
        "model",
        "effort",
        "approval_policy",
        "sandbox_mode",
        "collaboration_mode",
        "collaboration_instructions",
        "session_status",
        "mcp_startup",
        "session_lifecycle",
        "hook",
    ];
    let marker = |change: &str, turn: Option<Id>| json!({"marker":"harness_changed","data":{"program":"Untrusted display","change":change,"subject":"Native label","previous":{"state":"unsupported"},"value":{"status":"completed","private":"never-copy-runtime-body"},"expected":{"state":"unsupported"},"changed":true,"diverged":false,"next_turn":false,"turn_id":turn,"reported_at":world.clock.time()}});
    let complete = |reference: &str, payload: Json| {
        Event::decode(
            "item.completed",
            json!({"native_ref":reference,"status":"completed","payload":payload}),
        )
        .unwrap()
    };
    let mut frames = vec![
        delivered(
            &world,
            1,
            &Event::SessionOpened(SessionOpened {
                native_session_ref: "runtime-audit".into(),
                resumed: false,
            }),
        ),
        delivered(&world, 2, &turn_began(start.turn_id)),
    ];
    for (index, change) in changes.iter().enumerate() {
        frames.push(delivered(
            &world,
            3 + i64::try_from(index).unwrap(),
            &complete(change, marker(change, Some(start.turn_id))),
        ));
    }
    apply(&world, thread, segment, &frames).await;
    let rows:Vec<Json>=sqlx::query_scalar("SELECT jsonb_build_object('actor',actor_kind,'for_user',for_user_id,'details',details) FROM audit_log WHERE node_id=$1 AND action='harness.changed' ORDER BY at,id").bind(thread).fetch_all(world.pool.pool()).await.unwrap();
    assert_eq!(rows.len(), changes.len());
    for row in &rows {
        assert_eq!(row["actor"], "agent");
        assert_eq!(row["for_user"], json!(world.ada));
        assert_eq!(row["details"]["turn_id"], json!(start.turn_id));
        assert_ne!(row["details"]["program"], json!("Untrusted display"));
    }
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("never-copy-runtime-body")
    );
    let same = complete("model", marker("model", Some(start.turn_id)));
    apply(&world, thread, segment, &[delivered(&world, 20, &same)]).await;
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM audit_log WHERE node_id=$1 AND action='harness.changed'",
                thread
            )
            .await,
        10
    );
    let mut revised = marker("model", Some(start.turn_id));
    revised["data"]["value"] = json!("new-native-model");
    apply(
        &world,
        thread,
        segment,
        &[delivered(&world, 21, &complete("model", revised))],
    )
    .await;
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM audit_log WHERE node_id=$1 AND action='harness.changed'",
                thread
            )
            .await,
        11,
        "an actual update of one source item is audited"
    );
    let idle = complete("idle-observation", marker("session_status", None));
    apply(&world, thread, segment, &[delivered(&world, 22, &idle)]).await;
    let idle:Json=world.scalar("SELECT jsonb_build_object('actor',actor_kind,'for_user',for_user_id,'turn',details->'turn_id') FROM audit_log WHERE node_id=$1 AND action='harness.changed' AND details->>'native_ref'='idle-observation'",thread).await;
    assert_eq!(
        idle,
        json!({"actor":"system","for_user":null,"turn":null}),
        "idle facts borrow neither the newer turn nor its person"
    );
    let compacted = complete(
        "compaction-observation",
        json!({"marker":"compacted","data":{"duration_ms":null}}),
    );
    let rerouted = complete(
        "reroute-observation",
        json!({"code":"model_rerouted","message":"never-copy-runtime-body","retryable":false,"details":{"turn_id":start.turn_id,"from_model":"chosen","to_model":"native-effective"}}),
    );
    let review =
        orch_agent_contract::review::reported(orch_agent_contract::review::ReportedReview {
            native_ref: "review-observation".into(),
            turn: Some(start.turn_id),
            at: world.clock.time(),
            model: "codex-auto-review",
            model_reported: false,
            outcome: "approved",
            reason: Some("never-copy-runtime-body"),
            action: json!({"private":"never-copy-runtime-body"}),
            usage: None,
            raw: &Json::Null,
        });
    let review = review
        .into_iter()
        .find_map(|event| {
            if let orch_agent_contract::events::SessionEvent::ItemCompleted(item) = event {
                Some(complete("review-observation", item.payload))
            } else {
                None
            }
        })
        .unwrap();
    apply(
        &world,
        thread,
        segment,
        &[
            delivered(&world, 23, &compacted),
            delivered(&world, 24, &rerouted),
            delivered(&world, 25, &review),
        ],
    )
    .await;
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM audit_log WHERE node_id=$1 AND action='harness.changed'",
                thread
            )
            .await,
        15
    );
    let audit:Json=world.scalar("SELECT jsonb_agg(details ORDER BY at,id) FROM audit_log WHERE node_id=$1 AND action='harness.changed'",thread).await;
    assert!(!audit.to_string().contains("never-copy-runtime-body"));
    apply(
        &world,
        thread,
        segment,
        &[
            delivered(&world, 26, &compacted),
            delivered(&world, 27, &rerouted),
            delivered(&world, 28, &review),
        ],
    )
    .await;
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM audit_log WHERE node_id=$1 AND action='harness.changed'",
                thread
            )
            .await,
        15
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unscoped_program_status_is_a_provider_audit_fact_without_native_diagnostics() {
    let world = World::new().await;
    let thread = world.thread("Program status audit").await;
    world.running_turn(thread, "Go").await;
    let count = || async {
        world.count("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='harness.changed' AND details->>'change'='program_status'",world.provider).await
    };
    let before = count().await;
    let event = Event::HarnessStatus(orch_link_contract::events::HarnessStatus {
        status: "online".into(),
        detail: Some("never-copy-native-startup-words".into()),
        harness_version: None,
        sessions_open: 1,
    });
    for _ in 0..2 {
        let mut tx = world.pool.begin().await.unwrap();
        world
            .engine()
            .apply_stream_events(
                &mut tx,
                world.machine_id,
                &Stream::Harness {
                    provider_id: world.provider,
                    agent_id: world.agent,
                },
                &[delivered(&world, 1, &event)],
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(count().await, before + 1);
    let row:Json=world.scalar("SELECT jsonb_build_object('actor',actor_kind,'for_user',for_user_id,'object_kind',object_kind,'details',details) FROM audit_log WHERE object_id=$1 AND action='harness.changed' AND details->>'change'='program_status' ORDER BY at DESC,id DESC LIMIT 1",world.provider).await;
    assert_eq!(row["object_kind"], "provider");
    assert_eq!(row["actor"], "system");
    assert!(row["for_user"].is_null());
    assert!(row["details"]["thread_id"].is_null());
    assert!(row["details"]["turn_id"].is_null());
    assert!(!row.to_string().contains("never-copy-native-startup-words"));
}

#[tokio::test]
async fn retries_publish_each_native_attempt_once_and_clear_on_work_or_completion() {
    let world = World::new().await;
    let thread = world.thread("Retry phases").await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Please continue"),
        )
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    let segment = world.segment_of(thread).await;
    let began = delivered(&world, 1, &turn_began(start.turn_id));
    apply(&world, thread, segment, &[began]).await;
    for (seq, attempt) in [(2, 1), (3, 2), (4, 4)] {
        let event = delivered(
            &world,
            seq,
            &Event::TurnPhase(TurnPhase {
                turn_id: start.turn_id,
                kind: TurnPhaseKind::Retrying,
                retry_attempt: Some(attempt),
                at: Time::new(world.clock.now()),
            }),
        );
        apply(&world, thread, segment, std::slice::from_ref(&event)).await;
        let sync = world.sync(thread).await;
        let phase = sync
            .turns
            .iter()
            .find(|t| t.id == start.turn_id)
            .unwrap()
            .phase
            .as_ref()
            .unwrap();
        assert_eq!(
            phase.kind,
            orch_api_contract::threads::TurnPhaseKind::Retrying
        );
        assert_eq!(phase.retry_attempt, Some(attempt));
        let once = picture(&world, thread).await;
        apply(&world, thread, segment, &[event]).await;
        assert_eq!(
            picture(&world, thread).await,
            once,
            "replay does not reset the status timer"
        );
    }
    let working = delivered(
        &world,
        5,
        &Event::TurnPhase(TurnPhase {
            turn_id: start.turn_id,
            kind: TurnPhaseKind::Working,
            retry_attempt: Some(99),
            at: Time::new(world.clock.now()),
        }),
    );
    apply(&world, thread, segment, &[working]).await;
    let sync = world.sync(thread).await;
    let phase = sync
        .turns
        .iter()
        .find(|t| t.id == start.turn_id)
        .unwrap()
        .phase
        .as_ref()
        .unwrap();
    assert_eq!(
        phase.kind,
        orch_api_contract::threads::TurnPhaseKind::Working
    );
    assert_eq!(
        phase.retry_attempt, None,
        "only native retry phases can carry retry numbers"
    );
    let completed = delivered(
        &world,
        6,
        &turn_ended(start.turn_id, TurnOutcome::Completed),
    );
    apply(&world, thread, segment, &[completed]).await;
    let late = delivered(
        &world,
        7,
        &Event::TurnPhase(TurnPhase {
            turn_id: start.turn_id,
            kind: TurnPhaseKind::Retrying,
            retry_attempt: Some(6),
            at: Time::new(world.clock.now()),
        }),
    );
    apply(&world, thread, segment, &[late]).await;
    assert!(
        world
            .sync(thread)
            .await
            .turns
            .iter()
            .find(|t| t.id == start.turn_id)
            .unwrap()
            .phase
            .is_none(),
        "late status cannot reopen a completed turn"
    );
}

#[path = "ingest/claude_audit.rs"]
mod claude_audit;

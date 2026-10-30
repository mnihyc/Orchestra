//! Replayed harness frames are retained once by their machine sequence, while distinct equal frames remain distinct.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::World;
use orch_link::streams::DeliveredEvent;
use orch_link_contract::events::{Event, RawFrame};
use orch_link_contract::streams::Stream;
use orch_types::{Id, Time};
use serde_json::json;

async fn deliver(world: &World, thread: Id, segment: Id, events: &[DeliveredEvent]) {
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

#[tokio::test]
async fn a_frame_delivered_twice_is_kept_once_and_a_new_frame_with_the_same_json_is_kept() {
    let world = World::new().await;
    let thread = world.thread("Raw replay").await;
    world.running_turn(thread, "Keep the frames").await;
    let segment = world.segment_of(thread).await;
    let frame = Event::RawFrame(RawFrame {
        n: 7,
        direction: "in".to_owned(),
        payload: json!({ "method": "frame", "authorization": "private credential" }),
    });
    let first = DeliveredEvent {
        seq: 41,
        at: Time::new(world.clock.now()),
        kind: frame.kind().as_str().to_owned(),
        body: frame.body().unwrap(),
    };
    deliver(&world, thread, segment, std::slice::from_ref(&first)).await;
    deliver(&world, thread, segment, std::slice::from_ref(&first)).await;
    // Identity is the captured number, independent of both delivery sequence and reported moment.
    let mut renumbered_delivery = first.clone();
    renumbered_delivery.seq = 42;
    deliver(
        &world,
        thread,
        segment,
        std::slice::from_ref(&renumbered_delivery),
    )
    .await;
    let mut later = first.clone();
    later.at = Time::new(first.at.get() + chrono::Duration::days(32));
    deliver(&world, thread, segment, std::slice::from_ref(&later)).await;
    later.seq = 44;
    later.body["n"] = json!(8);
    deliver(&world, thread, segment, std::slice::from_ref(&later)).await;
    let mut conn = world.pool.acquire().await.unwrap();
    let rows = orch_db::queries::raw_events::newest(&mut conn, segment, None, 10)
        .await
        .unwrap();
    assert_eq!(rows.iter().map(|row| row.n).collect::<Vec<_>>(), [8, 7]);
    assert_eq!(
        orch_db::queries::raw_events::last_number(&mut conn, segment)
            .await
            .unwrap(),
        8
    );
    for row in rows {
        let payload: serde_json::Value =
            serde_json::from_slice(&zstd::stream::decode_all(row.payload.as_slice()).unwrap())
                .unwrap();
        assert_eq!(payload["method"], "frame");
        assert_eq!(
            payload["authorization"], "[masked]",
            "masking still precedes retention"
        );
    }
}

#[tokio::test]
async fn a_reopened_session_starts_above_the_segment_cursor_after_all_frame_bytes_are_pruned() {
    use orch_link_contract::commands::{Command, CommandKind};
    use orch_link_contract::events::TurnOutcome;
    let world = World::new().await;
    let thread = world.thread("Raw cursor survives retention").await;
    let (turn, _) = world.running_turn(thread, "First turn").await;
    let segment = world.segment_of(thread).await;
    let frame = Event::RawFrame(RawFrame {
        n: 71,
        direction: "in".to_owned(),
        payload: json!({"frame": "old"}),
    });
    let event = DeliveredEvent {
        seq: 41,
        at: Time::new(world.clock.now()),
        kind: "raw.frame".to_owned(),
        body: frame.body().unwrap(),
    };
    deliver(&world, thread, segment, std::slice::from_ref(&event)).await;
    world
        .say(
            thread,
            common::turn_ended(turn.turn_id, TurnOutcome::Completed),
        )
        .await;
    world.wait_turn(turn.turn_id, "completed").await;
    sqlx::query("DELETE FROM raw_events WHERE segment_id = $1")
        .bind(segment)
        .execute(world.pool.pool())
        .await
        .unwrap();
    // A fresh API process has no session cache; its opening uses the durable segment cursor.
    world.restart_api().await;
    world.running_turn(thread, "Second turn").await;
    let floors: Vec<_> = world
        .seen
        .of_kind(CommandKind::SessionOpen)
        .into_iter()
        .filter_map(|cmd| match cmd {
            Command::SessionOpen(open) if open.thread_id == thread => Some(open.raw_after),
            _ => None,
        })
        .collect();
    assert_eq!(floors.first(), Some(&0));
    assert_eq!(floors.last(), Some(&71));
    assert!(floors.len() >= 2, "the session actually reopened");
    let mut conn = world.pool.acquire().await.unwrap();
    deliver(
        &world,
        thread,
        segment,
        &[DeliveredEvent { seq: 99, ..event }],
    )
    .await;
    assert_eq!(
        orch_db::queries::raw_events::count(&mut conn, segment)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        orch_db::queries::raw_events::last_number(&mut conn, segment)
            .await
            .unwrap(),
        71
    );
}

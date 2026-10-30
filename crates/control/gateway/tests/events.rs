//! A machine's events arrive once each and in order, however the gateway and the system are restarted meanwhile.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{PATIENCE, World, eventually};
use orch_link_contract::enrollment::Trust;
use orch_link_contract::events::{Event, SessionTitle};
use orch_link_contract::streams::Stream;
use orch_types::new_id;
use std::time::Duration;

const EVENTS: i64 = 1_000;

#[tokio::test]
async fn a_thousand_events_arrive_once_and_in_order_while_the_gateway_and_the_api_are_each_restarted()
 {
    let mut world = World::start().await;
    let enrolled = world.enroll("alice-laptop").await;
    let machine = world.start_machine(&enrolled, enrolled.endpoints(Trust::Pinned));
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .expect("the machine reached the gateway");
    let first_session = world.api.link.session_of(machine.id);
    assert!(first_session.is_some());

    let stream = Stream::Session {
        thread_id: new_id(),
        segment_id: new_id(),
    };
    // Session reports need the same immutable source binding as a real session.open. Random
    // unregistered stream ids must be acknowledged and discarded under the source-ownership rule.
    if let Stream::Session {
        thread_id,
        segment_id,
    } = stream
    {
        use orch_db::queries::{
            hierarchy::testing::insert_node, resources::access_fixtures, threads,
        };
        let mut conn = world.db.acquire().await.unwrap();
        let (org, project, topic) = (new_id(), new_id(), new_id());
        insert_node(&mut conn, org, "org", None, "Replay team", world.user)
            .await
            .unwrap();
        insert_node(
            &mut conn,
            project,
            "project",
            Some(org),
            "Replay project",
            world.user,
        )
        .await
        .unwrap();
        insert_node(
            &mut conn,
            topic,
            "topic",
            Some(project),
            "Replay topic",
            world.user,
        )
        .await
        .unwrap();
        assert!(
            threads::insert_thread(
                &mut conn,
                thread_id,
                topic,
                "Replay",
                world.user,
                world.clock.now()
            )
            .await
            .unwrap()
        );
        let agent = access_fixtures::insert_resource(
            &mut conn,
            "agent",
            Some(world.user),
            Some(org),
            "Replay agent",
        )
        .await
        .unwrap();
        let provider = access_fixtures::insert_resource(
            &mut conn,
            "provider",
            Some(world.user),
            Some(org),
            "Replay provider",
        )
        .await
        .unwrap();
        let environment = access_fixtures::insert_environment(
            &mut conn,
            world.user,
            Some(org),
            "Replay environment",
            "connected",
        )
        .await
        .unwrap();
        threads::insert_segment(
            &mut conn,
            &threads::NewSegment {
                id: segment_id,
                thread_id,
                agent_id: agent,
                provider_id: provider,
                environment_id: environment,
                env_generation: 1,
                arrangement: "together",
                executor: None,
                tool_set: None,
                credential_path: "native",
                native_session_ref: None,
                started_by: Some(world.user),
                started_at: world.clock.now(),
            },
            1,
        )
        .await
        .unwrap();
        assert!(
            orch_db::queries::machine_streams::bind_segment(
                &mut conn, thread_id, segment_id, machine.id
            )
            .await
            .unwrap()
        );
    }
    // The machine keeps publishing at a steady pace, so that events are
    // made while the gateway is away and while the system is.
    let publishing = {
        let (link, stream) = (machine.link.clone(), stream);
        tokio::spawn(async move {
            for i in 0..EVENTS {
                link.publish(
                    &stream,
                    &Event::SessionTitle(SessionTitle {
                        title: format!("title {i}"),
                    }),
                )
                .await
                .unwrap();
                tokio::time::sleep(Duration::from_millis(4)).await;
            }
        })
    };
    tokio::time::sleep(Duration::from_millis(1_000)).await;
    world.restart_gateway().await;
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    world.restart_api().await;
    publishing.await.unwrap();

    let name = stream.to_string();
    let machine_id = machine.id;
    eventually("the thousand events to be applied", || {
        let db = world.db.clone();
        let name = name.clone();
        async move {
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM idempotency_keys WHERE method='LINK_EVENT' AND response->>'machine_id'=$1::uuid::text AND path=$2",
            )
            .bind(machine_id)
            .bind(&name)
            .fetch_one(db.pool())
            .await
            .unwrap();
            count == EVENTS
        }
    })
    .await;

    let rows: Vec<(i64, serde_json::Value)> = sqlx::query_as(
        "SELECT (response->>'seq')::bigint, response->'body' FROM idempotency_keys WHERE method='LINK_EVENT' AND response->>'machine_id'=$1::uuid::text AND path=$2 ORDER BY (response->>'seq')::bigint",
    )
    .bind(machine_id)
    .bind(&name)
    .fetch_all(world.db.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), usize::try_from(EVENTS).unwrap());
    for (place, (seq, body)) in rows.iter().enumerate() {
        assert_eq!(*seq, i64::try_from(place).unwrap() + 1, "without a hole");
        assert_eq!(
            body["title"],
            format!("title {place}"),
            "in the order they were made"
        );
    }
    assert!(
        world.applied_before > 0
            && world
                .api
                .applier
                .applied
                .load(std::sync::atomic::Ordering::SeqCst)
                > 0,
        "both the API that was restarted and the one that took over applied events"
    );
    eventually("the machine to hold a session again", || {
        let (link, id) = (world.api.link.clone(), machine_id);
        async move { link.session_of(id).is_some() }
    })
    .await;
    assert_ne!(
        world.api.link.session_of(machine_id),
        first_session,
        "the gateway's restart cost the machine its session"
    );
    let applied_twice = world.refused_before
        + world
            .api
            .applier
            .refused
            .load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(applied_twice, 0, "no event was applied a second time");

    // The cursor in the database says the same, and so does the machine.
    eventually("the cursor to reach the last event", || {
        let (db, name) = (world.db.clone(), name.clone());
        async move {
            let mut conn = db.acquire().await.unwrap();
            orch_db::queries::link::cursors(&mut conn, machine_id)
                .await
                .unwrap()
                .get(&name)
                .copied()
                == Some(EVENTS)
        }
    })
    .await;
    world.stop().await;
}

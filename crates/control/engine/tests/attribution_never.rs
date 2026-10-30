//! Where a place's `collab.attribution` is `never`, the texts Orch composes for the agent name nobody: the line after
//! a Stop says a person stopped the turn, and forwarded words come from "a person" and are forwarded by "a person",
//! with their source and time kept. Under `auto` the same texts name the person as people are shown.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, queued, turn_ended};
use orch_api_contract::threads::{
    ContentPart, Delivery, Forwarded, ForwardedAuthor, ForwardedFrom, ForwardedType,
};
use orch_api_contract::users::{Avatar, UserKind, UserRef};
use orch_authz::Caller;
use orch_engine::inputs::InputRequest;
use orch_link_contract::events::TurnOutcome;
use orch_types::{Id, Time, new_id};
use serde_json::json;

/// A thread whose last turn ada stopped, idle again: the next words are told why it was cut.
async fn stopped_by_ada(world: &World, name: &str) -> Id {
    let thread = world.thread(name).await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    world
        .engine()
        .interrupt(&Caller::session(world.ada), thread, start.turn_id)
        .await
        .unwrap();
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Interrupted))
        .await;
    world.wait_turn(start.turn_id, "interrupted").await;
    world.wait_state(thread, "idle").await;
    thread
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn under_never_the_line_after_a_stop_names_nobody_and_under_auto_it_names_her() {
    let world = World::new().await;
    world
        .set(world.topic, "collab.attribution", json!("never"))
        .await;
    let thread = stopped_by_ada(&world, "Nobody named").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Next"))
        .await
        .unwrap();
    let next = world.run_turn(thread, 1, "Done").await;
    assert_eq!(
        common::words_after_where(&next.text),
        "<orch>Your last turn was stopped by a person. What it was doing may be half done.</orch>\n\nNext"
    );

    world
        .set(world.topic, "collab.attribution", json!("auto"))
        .await;
    let thread = stopped_by_ada(&world, "Named").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Next"))
        .await
        .unwrap();
    let next = world.run_turn(thread, 1, "Done").await;
    assert_eq!(
        common::words_after_where(&next.text),
        "<orch>Your last turn was stopped by ada (@ada). What it was doing may be half done.</orch>\n\nNext"
    );
}

/// Words ada wrote in another thread, forwarded.
fn forwarded_from_ada(world: &World) -> InputRequest {
    InputRequest {
        plan: false,
        content: vec![ContentPart::Forwarded(Box::new(Forwarded {
            r#type: ForwardedType::Forwarded,
            from: serde_json::from_value::<ForwardedFrom>(json!({
                "kind": "thread",
                "thread": { "id": new_id(), "kind": "thread", "name": "Team", "redacted": false }
            }))
            .unwrap(),
            source_id: new_id(),
            author: Some(ForwardedAuthor::User(UserRef {
                id: world.ada,
                kind: UserKind::Human,
                username: "ada".into(),
                address: None,
                display_name: "ada".into(),
                avatar: Avatar {
                    text: "A".into(),
                    color: 3,
                },
            })),
            at: Time::new(chrono::Utc::now()),
            text: "The login fails after midnight.".into(),
            images: Vec::new(),
            attachments: Vec::new(),
        }))],
        delivery: Delivery::Queue,
        client_id: new_id().to_string(),
        bypass: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn under_never_forwarded_words_carry_no_name_and_under_auto_they_name_her() {
    let world = World::new().await;
    world
        .set(world.topic, "collab.attribution", json!("never"))
        .await;
    let thread = world.thread("Nobody named").await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            forwarded_from_ada(&world),
        )
        .await
        .unwrap();
    let start = world.run_turn(thread, 0, "Done").await;
    let said = common::words_after_where(&start.text);
    assert!(
        said.starts_with(
            "<relayed from-person=\"a person\" kind=\"forwarded\" by=\"a person\" source=\"Team\" at=\""
        ),
        "{said}"
    );
    assert!(said.contains("The login fails after midnight."), "{said}");
    assert!(!said.contains("ada"), "nobody is named: {said}");

    world
        .set(world.topic, "collab.attribution", json!("auto"))
        .await;
    let thread = world.thread("Named").await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            forwarded_from_ada(&world),
        )
        .await
        .unwrap();
    let start = world.run_turn(thread, 0, "Done").await;
    let said = common::words_after_where(&start.text);
    assert!(
        said.starts_with(
            "<relayed from-person=\"ada (@ada)\" kind=\"forwarded\" by=\"ada (@ada)\" source=\"Team\" at=\""
        ),
        "{said}"
    );
}

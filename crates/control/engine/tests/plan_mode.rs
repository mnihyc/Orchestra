//! Plan mode: a message sent with the composer's Plan on is worked as a plan turn by a program that declares plan
//! mode, refused before it is sent where the program has none, and never steers a turn of the other kind. A plan
//! the agent proposes is answered by whoever may write to the agent, unless the place keeps plans to managers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, queued, steering};
use orch_api_contract::threads::InputState;
use orch_authz::Caller;
use orch_engine::inputs::{InputError, InputRequest};
use serde_json::json;

fn planned(text: &str) -> InputRequest {
    InputRequest {
        plan: true,
        ..queued(text)
    }
}

const PLANS: &str =
    r#"UPDATE agents SET descriptor_overrides = '{"capabilities":{"plan_mode":true}}'"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plan_is_refused_where_the_program_cannot_plan_and_is_otherwise_a_plan_turn() {
    let world = World::new().await;
    let thread = world.thread("Planned").await;
    let ada = Caller::session(world.ada);
    let refused = world
        .engine()
        .submit_input(&ada, thread, planned("Plan the redirect fix"))
        .await;
    assert!(
        matches!(refused, Err(InputError::PlanUnsupported)),
        "{refused:?}"
    );
    world.exec(PLANS).await;
    let taken = world
        .engine()
        .submit_input(&ada, thread, planned("Plan the redirect fix"))
        .await
        .unwrap();
    assert!(taken.input.plan);
    let start = world.turn_start(thread, 0).await;
    assert!(start.plan, "the turn is worked in plan mode");
    // An ordinary message after it is an ordinary turn: planning never sticks to the thread.
    world.run_turn(thread, 0, "Here is the plan").await;
    world
        .engine()
        .submit_input(&ada, thread, queued("Implement the plan."))
        .await
        .unwrap();
    assert!(!world.turn_start(thread, 1).await.plan);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn words_for_work_wait_for_their_own_turn_instead_of_steering_a_plan() {
    let world = World::new().await;
    world.exec(PLANS).await;
    let thread = world.thread("Planning").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&ada, thread, planned("Plan the redirect fix"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    world.say(thread, common::turn_began(start.turn_id)).await;
    world.wait_turn(start.turn_id, "running").await;
    let work = world
        .engine()
        .submit_input(&ada, thread, steering("Change the handler now"))
        .await
        .unwrap();
    assert_eq!(
        work.input.state,
        InputState::Pending,
        "it waits rather than steering"
    );
    let more = world
        .engine()
        .submit_input(
            &ada,
            thread,
            InputRequest {
                plan: true,
                ..steering("Also cover the logout path")
            },
        )
        .await
        .unwrap();
    assert_eq!(
        more.input.state,
        InputState::Delivering,
        "words for the plan go into it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_place_decides_whether_writers_or_only_managers_answer_a_plan() {
    let world = World::new().await;
    let thread = world.thread("Answered").await;
    let plan = json!({ "request_kind": "plan", "detail": { "plan": "1. Read" }, "options": [] });
    let command = json!({ "request_kind": "command", "detail": { "command": "ls", "cwd": "/w", "reason": null }, "options": [] });
    let mut conn = world.pool.acquire().await.unwrap();
    assert_eq!(
        orch_engine::approvals::deciding_permission(&mut conn, thread, &plan)
            .await
            .unwrap(),
        orch_access_contract::permissions::Permission::ThreadInteract
    );
    assert_eq!(
        orch_engine::approvals::deciding_permission(&mut conn, thread, &command)
            .await
            .unwrap(),
        orch_access_contract::permissions::Permission::ThreadManage
    );
    drop(conn);
    world
        .set(world.topic, "thread.plan_answered_by", json!("managers"))
        .await;
    let mut conn = world.pool.acquire().await.unwrap();
    assert_eq!(
        orch_engine::approvals::deciding_permission(&mut conn, thread, &plan)
            .await
            .unwrap(),
        orch_access_contract::permissions::Permission::ThreadManage
    );
}

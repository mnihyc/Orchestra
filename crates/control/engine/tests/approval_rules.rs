//! One narrow rule model: an action allowed for a thread, held to the matcher its adapter verifies, answers a later
//! request offering the same matcher once, is taken back by a person, and is never answered by a mode or kept from one.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::World;
use orch_api_contract::approvals::{ApprovalDecision, HarnessOptionDecision};
use orch_authz::{Caller, Principal};
use orch_link_contract::events::Event;
use orch_types::{Id, Time};
use serde_json::json;

/// A request to run a command that offers, beside Allow once and Deny, Allow for this thread with its matcher.
fn asked_with_rule(turn: Id, request_ref: &str) -> Event {
    Event::ApprovalRequested(orch_link_contract::events::ApprovalRequested {
        turn_id: turn,
        request_ref: request_ref.to_owned(),
        title: "Run the tests".to_owned(),
        request: serde_json::from_value(json!({
            "request_kind": "command",
            "detail": { "command": "cargo test -p web", "cwd": "/workspace/web", "reason": null },
            "options": [
                { "id": "allow_once", "label": "Allow once", "style": "allow", "scope": "once", "rule": null },
                { "id": "allow_thread", "label": "Allow for this thread", "style": "allow", "scope": "thread",
                  "rule": { "type": "command_prefix", "argv": ["cargo", "test"], "cwd": "/workspace/web" } },
                { "id": "deny", "label": "Deny", "style": "deny", "scope": null, "rule": null }
            ]
        })).unwrap(),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_action_allowed_for_a_thread_answers_the_same_request_once_until_it_is_taken_back() {
    let world = World::new().await;
    let thread = world.thread("Tests").await;
    let (start, _) = world.running_turn(thread, "Run the tests").await;
    world
        .say(thread, asked_with_rule(start.turn_id, "req-1"))
        .await;
    world.wait_state(thread, "waiting_approval").await;
    let first: Id = world
        .scalar("SELECT id FROM approvals WHERE thread_id = $1", thread)
        .await;
    world
        .engine()
        .decide_approval(
            &Caller::session(world.ada),
            first,
            &ApprovalDecision::Option(HarnessOptionDecision {
                option_id: "allow_thread".to_owned(),
                note: None,
            }),
        )
        .await
        .unwrap();
    let rule: Id = world
        .scalar(
            "SELECT id FROM thread_approval_rules WHERE thread_id = $1 AND revoked_at IS NULL",
            thread,
        )
        .await;
    world.wait_state(thread, "running").await;

    // The same request again: answered once by the rule, with the request's own Allow once, and nobody asked.
    world
        .say(thread, asked_with_rule(start.turn_id, "req-2"))
        .await;
    common::eventually("the second request answered by the rule", || {
        let world = &world;
        async move {
            world
                .count(
                    "SELECT count(*) FROM approvals WHERE thread_id = $1 AND native_ref = 'req-2' AND state = 'approved' AND decided_by IS NULL",
                    thread,
                )
                .await
                == 1
        }
    })
    .await;
    let chosen: serde_json::Value = world
        .scalar(
            "SELECT decision FROM approvals WHERE thread_id = $1 AND native_ref = 'req-2'",
            thread,
        )
        .await;
    assert_eq!(
        chosen["option_id"], "allow_once",
        "a rule answers once, never with a lasting option"
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM audit_log WHERE action = 'approval.decided' AND details->>'rule' IS NOT NULL AND node_id = $1",
                thread
            )
            .await,
        1,
        "the audit names the rule that answered"
    );

    // Taken back: the next request waits for a person again.
    let mut tx = world.pool.begin().await.unwrap();
    let principal = Principal::load(
        &mut tx,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    world
        .engine()
        .revoke_approval_rule_guarded(tx, principal, thread, rule)
        .await
        .unwrap();
    world
        .say(thread, asked_with_rule(start.turn_id, "req-3"))
        .await;
    world.wait_state(thread, "waiting_approval").await;
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM approvals WHERE thread_id = $1 AND native_ref = 'req-3' AND state = 'pending'",
                thread
            )
            .await,
        1
    );
}

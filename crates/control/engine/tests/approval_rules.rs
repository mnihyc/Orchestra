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

fn crafted_request(turn: Id, reference: &str, kind: &str, detail: serde_json::Value) -> Event {
    Event::ApprovalRequested(orch_link_contract::events::ApprovalRequested {
        turn_id: turn,
        request_ref: reference.into(),
        title: "A request with a matching option".into(),
        request: serde_json::from_value(json!({
            "request_kind":kind, "detail":detail,
            "options":[
                {"id":"allow_once","label":"Allow once","style":"allow","scope":"once"},
                {"id":"allow_thread","label":"Allow for this thread","style":"allow","scope":"thread",
                 "rule":{"type":"command_prefix","argv":["cargo","test"],"cwd":"/workspace/web"}}
            ]
        })).unwrap(),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plan_writer_cannot_create_a_lasting_permission_rule() {
    let world = World::new().await;
    let writer = world.person("plan-writer").await;
    world.give(writer, "interactor").await;
    let thread = world.thread("Plan cannot grant a rule").await;
    let (start, _) = world.running_turn(thread, "Plan the work").await;
    let mut request = match crafted_request(
        start.turn_id,
        "crafted-plan",
        "plan",
        json!({"plan":"Read and then change"}),
    ) {
        Event::ApprovalRequested(request) => request,
        _ => unreachable!(),
    };
    request.request.options[1].id = "proceed".into();
    world.say(thread, Event::ApprovalRequested(request)).await;
    world.wait_state(thread, "waiting_approval").await;
    let approval: Id = world
        .scalar("SELECT id FROM approvals WHERE thread_id=$1", thread)
        .await;
    world
        .engine()
        .decide_approval(
            &Caller::session(writer),
            approval,
            &ApprovalDecision::Option(HarnessOptionDecision {
                option_id: "proceed".into(),
                note: None,
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM thread_approval_rules WHERE thread_id=$1",
                thread
            )
            .await,
        0
    );
    world
        .say(thread, asked_with_rule(start.turn_id, "command-after-plan"))
        .await;
    common::eventually("the later command waits for a manager", || async {
        world.count("SELECT count(*) FROM approvals WHERE thread_id=$1 AND native_ref='command-after-plan' AND state='pending'", thread).await == 1
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_permission_rule_does_not_answer_questions_forms_plans_files_or_unknown_requests()
{
    use orch_link_contract::commands::CommandKind;
    let world = World::new().await;
    let thread = world
        .thread("A rule only answers its permission request")
        .await;
    let (start, _) = world.running_turn(thread, "Run the tests").await;
    world
        .say(thread, asked_with_rule(start.turn_id, "rule-origin"))
        .await;
    world.wait_state(thread, "waiting_approval").await;
    let approval: Id = world
        .scalar("SELECT id FROM approvals WHERE thread_id=$1", thread)
        .await;
    world
        .engine()
        .decide_approval(
            &Caller::session(world.ada),
            approval,
            &ApprovalDecision::Option(HarnessOptionDecision {
                option_id: "allow_thread".into(),
                note: None,
            }),
        )
        .await
        .unwrap();
    world.wait_ran(CommandKind::ApprovalResolve, 1).await;
    let kinds = [
        (
            "user_input",
            json!({"questions":[{"id":"q","header":"Choice","question":"Which?","multiple":false,"allow_other":true,"options":null}]}),
        ),
        (
            "mcp_elicitation",
            json!({"server":"example","message":"Choose","schema":{"type":"object"}}),
        ),
        ("plan", json!({"plan":"The plan"})),
        ("file_change", json!({"changes":[],"reason":null})),
        ("other", json!({"summary":"Unknown action","raw":{}})),
    ];
    for (kind, detail) in kinds {
        world
            .say(thread, crafted_request(start.turn_id, kind, kind, detail))
            .await;
        common::eventually("the request is recorded", || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM approvals WHERE thread_id=$1 AND native_ref=$2)",
            )
            .bind(thread)
            .bind(kind)
            .fetch_one(world.pool.pool())
            .await
            .unwrap()
        })
        .await;
        let state: String =
            sqlx::query_scalar("SELECT state FROM approvals WHERE thread_id=$1 AND native_ref=$2")
                .bind(thread)
                .bind(kind)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        assert_eq!(state, "pending", "{kind} must wait for a person");
    }
    assert_eq!(world.commands.issued_of("approval.resolve"), 1);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_request_after_stop_cannot_be_approved_by_an_existing_thread_rule() {
    use orch_link_contract::commands::CommandKind;
    use orch_link_contract::streams::Stream;
    let world = World::new().await;
    let thread = world.thread("Stop before a matching request").await;
    let (start, _) = world.running_turn(thread, "Run the tests").await;
    world
        .say(thread, asked_with_rule(start.turn_id, "before-stop"))
        .await;
    world.wait_state(thread, "waiting_approval").await;
    let first: Id = world
        .scalar("SELECT id FROM approvals WHERE thread_id=$1", thread)
        .await;
    world
        .engine()
        .decide_approval(
            &Caller::session(world.ada),
            first,
            &ApprovalDecision::Option(HarnessOptionDecision {
                option_id: "allow_thread".into(),
                note: None,
            }),
        )
        .await
        .unwrap();
    world.wait_ran(CommandKind::ApprovalResolve, 1).await;
    world.machine.handler.hold(CommandKind::TurnInterrupt);
    world
        .engine()
        .interrupt(&Caller::session(world.ada), thread, start.turn_id)
        .await
        .unwrap();
    let segment = world.segment_of(thread).await;
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: segment,
    };
    let name = stream.to_string();
    let before = {
        let mut conn = world.pool.acquire().await.unwrap();
        orch_db::queries::link::cursors(&mut conn, world.machine_id)
            .await
            .unwrap()
            .get(&name)
            .copied()
            .unwrap_or(0)
    };
    world
        .say(thread, asked_with_rule(start.turn_id, "after-stop"))
        .await;
    common::eventually("the late request was consumed", || async {
        let mut conn = world.pool.acquire().await.unwrap();
        orch_db::queries::link::cursors(&mut conn, world.machine_id)
            .await
            .unwrap()
            .get(&name)
            .copied()
            .unwrap_or(0)
            > before
    })
    .await;
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM approvals WHERE thread_id=$1 AND native_ref='after-stop'",
                thread
            )
            .await,
        0
    );
    assert_eq!(world.commands.issued_of("approval.resolve"), 1);
    world.machine.handler.release();
}

//! Published-protocol additions travel through the actual byte codec and control actor, without a binary.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]
mod common;

use common::corpus::{Actor, WAIT, barrier, from_native, handshake, input, to_native, turn};
use orch_adapter_claude_code::{conversation::Command, sessions::Origin};
use orch_agent_contract::session::ApprovalDecision;
use orch_testkit::harness_corpus::{Frame, assert_unrecognized, collect_through};
use serde_json::json;

#[tokio::test]
async fn future_stream_methods_are_counted_while_active_or_idle_without_admitting_work_or_private_data()
 {
    for idle in [false, true] {
        let mut frames = handshake();
        if !idle {
            frames.push(input());
        }
        // This known opaque metadata remains quiet; only the future event names below are unrecognized.
        frames.push(from_native(json!({"type":"stream_event","event":{"type":"content_block_delta",
            "index":0,"delta":{"type":"signature_delta","signature":"NEVER PUBLISH THIS SIGNATURE"}}})));
        for uuid in ["future-stream-1", "future-stream-2"] {
            frames.push(from_native(json!({"type":"stream_event","uuid":uuid,
                "event":{"type":"future_update","private_body":"NEVER PUBLISH THIS BODY"}})));
        }
        frames.extend(barrier());
        let mut actor = Actor::start(frames, Origin::New, idle).await;
        actor.opened.as_ref().unwrap();
        let observed = actor.collect().await;
        assert_unrecognized(&observed, "Claude Code", "stream_event/future_update", 2).unwrap();
        let completed: Vec<_> = observed
            .iter()
            .filter(|row| row.name == "item.completed")
            .collect();
        assert_eq!(completed.len(), 2);
        for row in completed {
            assert_eq!(
                row.body["payload"]["data"]["turn_id"],
                if idle { json!(null) } else { json!(turn()) }
            );
            assert_eq!(row.body["payload"]["data"]["change"], "unrecognized_event");
        }
        assert!(!observed.iter().any(|row| matches!(
            row.name.as_str(),
            "turn.completed" | "turn.phase" | "usage.updated" | "session.lost" | "raw.frame"
        )));
        assert!(
            !serde_json::to_string(&observed)
                .unwrap()
                .contains("NEVER PUBLISH")
        );
        if idle {
            assert!(!observed.iter().any(|row| matches!(
                row.name.as_str(),
                "item.started" | "turn.started" | "model.request" | "background.tasks"
            )));
        }
        actor.close().await;
    }
}

#[tokio::test]
async fn unknown_events_and_methods_are_counted_without_raw_logging_or_an_invented_idle_turn() {
    for idle in [false, true] {
        let mut frames = handshake();
        if !idle {
            frames.push(input());
        }
        frames.push(Frame::Tick { milliseconds: 1000 });
        for uuid in ["future-1", "future-2"] {
            frames.push(from_native(
                json!({"type":"future_update","uuid":uuid,"private_body":"never publish"}),
            ));
        }
        frames.push(from_native(json!({"type":"control_request","request_id":"future-request","request":{"subtype":"future_method","private_body":"never publish"}})));
        frames.push(to_native(json!({"type":"control_response","response":{"subtype":"error","request_id":"future-request","error":"future_method is not something this host answers"}})));
        frames.extend(barrier());
        let mut actor = Actor::start(frames, Origin::New, idle).await;
        actor.opened.as_ref().unwrap();
        let observed = actor.collect().await;
        assert_unrecognized(&observed, "Claude Code", "future_update", 2).unwrap();
        assert_unrecognized(&observed, "Claude Code", "control_request/future_method", 1).unwrap();
        assert!(!observed.iter().any(|row| row.name == "raw.frame"));
        for row in observed.iter().filter(|row| row.name == "item.completed") {
            let data = &row.body["payload"]["data"];
            assert_eq!(
                data["turn_id"],
                if idle { json!(null) } else { json!(turn()) }
            );
            assert_eq!(data["reported_at"], "2026-10-08T14:00:01.000Z");
            assert!(!row.body.to_string().contains("never publish"));
        }
        if idle {
            assert!(
                !observed
                    .iter()
                    .any(|row| row.name == "item.started" || row.name == "turn.started")
            );
        }
        actor.close().await;
    }
}

#[tokio::test]
async fn a_malformed_native_line_is_a_visible_bounded_error_and_the_next_request_still_works() {
    for idle in [false, true] {
        let mut frames = handshake();
        if !idle {
            frames.push(input());
        }
        frames.push(Frame::Raw {
            line: format!("{{broken {}", "x".repeat(400)),
            newline: true,
        });
        frames.extend(barrier());
        let mut actor = Actor::start(frames, Origin::New, idle).await;
        actor.opened.as_ref().unwrap();
        let observed = actor.collect().await;
        let errors: Vec<_> = observed
            .iter()
            .filter(|row| row.name == "item.completed")
            .collect();
        assert_eq!(errors.len(), 1);
        let error = &errors[0].body["payload"];
        assert_eq!(error["code"], "harness_error");
        assert!(error["message"].as_str().unwrap().contains("{broken"));
        assert!(error["message"].as_str().unwrap().chars().count() < 260);
        assert_eq!(error["retryable"], false);
        assert!(
            !observed
                .iter()
                .any(|row| row.name == "turn.completed" || row.name == "session.lost")
        );
        if idle {
            assert!(!observed.iter().any(|row| row.name == "item.started"));
        }
        actor.close().await;
    }
}

#[tokio::test]
async fn an_unknown_native_tool_asks_with_its_real_choices_and_sends_the_exact_refusal() {
    let mut frames = handshake();
    frames.push(input());
    frames.push(from_native(json!({"type":"control_request","request_id":"future-permission","request":{"subtype":"can_use_tool","tool_name":"FutureTool","tool_use_id":"future-tool","input":{"operation":"fixture"},"description":"Future permission"}})));
    frames.push(to_native(json!({"type":"control_response","response":{"subtype":"success","request_id":"future-permission","response":{"behavior":"deny","message":"Corpus refusal"}}})));
    frames.extend(barrier());
    let mut actor = Actor::start(frames, Origin::New, false).await;
    actor.opened.as_ref().unwrap();
    let mut observed = collect_through(&mut actor.events, "approval.requested", WAIT)
        .await
        .unwrap();
    let request = &observed.last().unwrap().body["request"];
    assert_eq!(request["request_kind"], "other");
    assert_eq!(request["detail"]["summary"], "Future permission");
    assert_eq!(
        request["detail"]["raw"],
        json!({"tool":"FutureTool","input":{"operation":"fixture"}})
    );
    let options = request["options"].as_array().unwrap();
    assert_eq!(
        options
            .iter()
            .map(|option| option["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["allow_once", "deny"]
    );
    actor
        .handle
        .ask(|reply| Command::Resolve {
            request_ref: "future-permission".into(),
            decision: ApprovalDecision::Option {
                option_id: "deny".into(),
                note: Some("Corpus refusal".into()),
            },
            reply,
        })
        .await
        .unwrap()
        .unwrap();
    observed.extend(actor.collect().await);
    // The deciding host records this answer; only a native cancellation emits approval.closed.
    assert!(!observed.iter().any(|row| row.name == "approval.closed"));
    assert!(
        !observed
            .iter()
            .any(|row| row.name == "item.started" || row.name == "turn.completed")
    );
    actor.close().await;
}

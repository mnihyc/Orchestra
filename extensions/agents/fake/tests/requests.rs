//! One raw request per simulated model step; tool execution and parse failures do not invent calls.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Fake, model_requests, of, said};
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::requests::UsageRequest;
use orch_agent_contract::session::ApprovalDecision;
use orch_types::Clock;

fn completed(events: &[common::Seen]) -> Vec<UsageRequest> {
    let requests = model_requests(events);
    for request in &requests {
        request.check().unwrap();
    }
    requests
        .into_iter()
        .filter(|request| request.outcome.is_some())
        .collect()
}

#[tokio::test(start_paused = true)]
async fn reasoning_and_message_pieces_share_a_step_and_a_tool_result_starts_the_next() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .script(
            &session,
            &[
                "think Looking",
                "say Before",
                "run pwd => 0 done",
                "say After",
            ],
        )
        .await;
    let reports = model_requests(&events);
    let ended = completed(&events);
    assert_eq!(
        reports.len(),
        6,
        "asked, first output and end for each of the two model steps"
    );
    assert_eq!(ended.len(), 2);
    assert_ne!(ended[0].id, ended[1].id);
    assert_eq!(
        [
            ended[0].outcome.as_ref().unwrap().code(),
            ended[1].outcome.as_ref().unwrap().code()
        ],
        ["done", "done"]
    );
    assert!(
        reports
            .iter()
            .filter(|request| request.outcome.is_none())
            .all(|request| request.tokens.output_tokens.is_none())
    );
    assert!(
        ended
            .iter()
            .all(|request| request.tokens.output_tokens.is_none()),
        "output counts were not declared; they stay unknown"
    );
    assert!(of(&events, "model.request").iter().all(
        |event| event.thread == Some(session.thread) && event.segment == Some(session.segment)
    ));
    let starts: Vec<_> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| {
            event.is("model.request")
                && event.body["request"]["first_output"].is_null()
                && event.body["request"]["outcome"].is_null()
        })
        .map(|(index, _)| index)
        .collect();
    let tool = of(&events, "item.started")
        .into_iter()
        .find(|event| event.body["kind"] == "tool_call")
        .unwrap();
    let tool_end = events
        .iter()
        .position(|event| {
            event.is("item.completed") && event.body["native_ref"] == tool.body["native_ref"]
        })
        .unwrap();
    assert!(
        starts[1] > tool_end,
        "the second model step follows the local tool's completion"
    );
    assert_eq!(said(&events), ["Before", "After"]);
}

#[tokio::test(start_paused = true)]
async fn a_trailing_tool_has_no_phantom_followup_request_and_a_followup_failure_is_its_own_attempt()
{
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake.script(&session, &["run pwd => 0 done"]).await;
    let reports = model_requests(&events);
    let ended = completed(&events);
    assert_eq!(
        reports.len(),
        2,
        "asked and a buffered tool-call boundary, with no empty request after the tool"
    );
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].outcome.as_ref().unwrap().code(), "done");
    assert_eq!(ended[0].output_streamed, Some(false));
    let failed = fake.script(&session, &["run pwd => 0 done", "busy"]).await;
    let ended = completed(&failed);
    assert_eq!(ended.len(), 2);
    assert_eq!(ended[0].outcome.as_ref().unwrap().code(), "done");
    assert_eq!(ended[1].outcome.as_ref().unwrap().code(), "provider_busy");
    assert!(ended[1].first_output.is_none());
    assert!(ended[1].tokens.output_tokens.is_none());
}

#[tokio::test(start_paused = true)]
async fn valid_and_invalid_scripts_after_orchs_context_keep_the_same_model_step_rules() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let prefix = "<orch>\nWhere you are\n</orch>\n\n";
    let valid = fake
        .say(
            &session,
            &format!("{prefix}#!fake\nsay Visible\nusage 100 64"),
        )
        .await;
    assert_eq!(said(&valid), ["Visible"]);
    let ended = completed(&valid);
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].tokens.input_tokens, Some(100));
    assert_eq!(ended[0].tokens.output_tokens, Some(64));
    let invalid = fake
        .say(
            &session,
            &format!("{prefix}#!fake\nsay Never\ndance wildly"),
        )
        .await;
    assert!(model_requests(&invalid).is_empty());
    assert!(said(&invalid).is_empty());
    assert_eq!(
        of(&invalid, "turn.completed")[0].body["error"]["code"],
        "harness_error"
    );
    assert!(
        of(&invalid, "item.started")[0].body["payload"]["message"]
            .as_str()
            .unwrap()
            .contains("line 3")
    );
}

#[tokio::test(start_paused = true)]
async fn a_persons_approval_wait_is_between_model_steps_and_never_inside_a_request() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.begin(&session, "#!fake\nask command pwd\nsay After approval")
        .await;
    let mut events = fake.until(|event| event.is("approval.requested")).await;
    let before = completed(&events);
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].outcome.as_ref().unwrap().code(), "done");
    let request_ref = of(&events, "approval.requested")[0].body["request_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    fake.clock.advance(chrono::Duration::seconds(30));
    let resumed = fake.clock.time();
    fake.adapter
        .resolve_approval(
            session.thread,
            &request_ref,
            ApprovalDecision::Option {
                option_id: "once".into(),
                note: None,
            },
        )
        .await
        .unwrap();
    events.extend(fake.until(|event| event.is("turn.completed")).await);
    let ended = completed(&events);
    assert_eq!(
        ended.len(),
        2,
        "approving an already-decided command does not invent another model step"
    );
    assert!(ended[0].end.unwrap() < resumed);
    assert_eq!(ended[1].asked, Some(resumed));
    assert_eq!(ended[1].first_output, Some(resumed));
    assert_eq!(said(&events), ["After approval"]);
}

//! Upgrade controls exercise the real scripted adapter and its shared native observation vocabulary.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]
mod common;

use common::{Fake, model_requests, of, said};
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::events::{FileSearchMode, OutputRef, ToolPayload};
use orch_agent_contract::session::ApprovalDecision;
use orch_testkit::harness_corpus::{Observation, assert_unrecognized};
use serde_json::json;

#[tokio::test(start_paused = true)]
async fn an_idle_compaction_is_a_persisted_marker_without_an_invented_turn() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    assert!(
        fake.adapter
            .observe_idle(session.thread, json!({"change":" ","value":{}}))
            .await
            .is_err()
    );
    fake.adapter
        .observe_idle(
            session.thread,
            json!({"change":"compaction","value":{"duration_ms":0}}),
        )
        .await
        .unwrap();
    let event = fake.next().await;
    assert!(event.is("item.completed"));
    assert_eq!(event.body["payload"]["marker"], "compacted");
    assert_eq!(event.body["payload"]["data"]["duration_ms"], 0);
    let file = fake.file(&session.native_ref);
    assert!(file["turns"].as_array().unwrap().is_empty());
    assert_eq!(file["items"].as_array().unwrap().len(), 1);
    let turn = fake.begin(&session, "#!fake\nsleep 10000").await;
    fake.until(|event| event.is("turn.started")).await;
    assert!(
        fake.adapter
            .observe_idle(session.thread, json!({"change":"compaction","value":{}}))
            .await
            .is_err()
    );
    fake.adapter.interrupt(session.thread, turn).await.unwrap();
    fake.until(|event| event.is("turn.completed")).await;
}

#[tokio::test(start_paused = true)]
async fn unknown_frames_are_persisted_counted_observations_without_raw_logging_or_tool_acts() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .script(
            &session,
            &[
                "unknown future/update",
                "unknown future/update",
                "say kept working",
            ],
        )
        .await;
    let rows = events
        .iter()
        .map(|event| Observation {
            name: event.name.clone(),
            body: event.body.clone(),
        })
        .collect::<Vec<_>>();
    assert_unrecognized(&rows, "Fake", "future/update", 2).unwrap();
    assert!(!events.iter().any(|event| event.is("raw.frame")));
    assert!(
        !events
            .iter()
            .any(|event| event.is("item.started") && event.body["kind"] == "tool_call")
    );
    assert_eq!(said(&events), vec!["kept working"]);
    assert_eq!(events.last().unwrap().body["status"], "completed");
}

#[tokio::test(start_paused = true)]
async fn typed_read_search_web_and_unknown_tools_keep_their_actual_neutral_payloads() {
    let tools = [
        ToolPayload::FileRead {
            path: "fixture.txt".into(),
            start_line: Some(1),
            end_line: Some(2),
            total_lines: Some(2),
            content: OutputRef::of("one\ntwo"),
            media_type: None,
        },
        ToolPayload::FileSearch {
            mode: FileSearchMode::Content,
            query: "fixture".into(),
            path: Some("/workspace".into()),
            matches: vec![],
            total: Some(0),
            truncated: false,
        },
        ToolPayload::WebSearch {
            action: None,
            query: "synthetic query".into(),
            results: vec![],
        },
        ToolPayload::WebFetch {
            url: "https://page.example.test".into(),
            title: Some("Fixture".into()),
            status: Some(200),
            content: OutputRef::of("fixture page"),
        },
        ToolPayload::Other {
            name: "future_tool".into(),
            arguments: Some(json!({"extra":true})),
            result: Some(json!({"kept":true})),
        },
    ];
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    for tool in tools {
        let line = format!("tool {}", serde_json::to_string(&tool).unwrap());
        let events = fake.script(&session, &[line.as_str()]).await;
        let complete = of(&events, "item.completed")
            .into_iter()
            .find(|event| event.body["payload"].get("tool").is_some())
            .unwrap();
        assert_eq!(
            complete.body["payload"]["tool"],
            serde_json::to_value(tool).unwrap()
        );
        assert_eq!(complete.body["status"], "completed");
    }
}

#[tokio::test(start_paused = true)]
async fn every_native_request_family_reaches_its_card_and_interrupts_through_the_existing_wait() {
    for (kind, detail) in [
        (
            "command",
            json!({"command":"synthetic command","cwd":"/workspace","reason":null}),
        ),
        ("file_change", json!({"changes":[],"reason":null})),
        (
            "permissions",
            json!({"permissions":{"network":false},"reason":null}),
        ),
        ("plan", json!({"plan":"A synthetic plan"})),
        (
            "user_input",
            json!({"questions":[{"id":"q","header":"Question","question":"Choose a fixture","multiple":false,"allow_other":true,"options":null}]}),
        ),
        (
            "mcp_elicitation",
            json!({"server":"fixture","message":"Enter fixture input","schema":{"type":"object","properties":{}}}),
        ),
        (
            "other",
            json!({"summary":"Future request","raw":{"kind":"future"}}),
        ),
    ] {
        let mut fake = Fake::start().await;
        let session = fake.session().await;
        let line = format!(
            "#!fake\nrequest {}",
            json!({"request_kind":kind,"detail":detail,"options":[{"id":"deny","label":"Cancel","style":"deny","scope":null,"rule":null}]})
        );
        let turn = fake.begin(&session, &line).await;
        let prefix = fake.until(|event| event.is("approval.requested")).await;
        assert_eq!(prefix.last().unwrap().body["request"]["request_kind"], kind);
        fake.adapter.interrupt(session.thread, turn).await.unwrap();
        let rest = fake.until(|event| event.is("turn.completed")).await;
        assert_eq!(rest.last().unwrap().body["status"], "interrupted");
    }
}

#[tokio::test(start_paused = true)]
async fn an_unknown_request_with_no_native_answer_is_a_generic_card_with_real_cancellation() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.begin(&session,"#!fake\nrequest {\"request_kind\":\"future_approval\",\"options\":[],\"detail\":{\"future\":true}}\nsay must not run").await;
    let events = fake.until(|event| event.is("approval.requested")).await;
    let request = &events.last().unwrap().body;
    assert_eq!(request["request"]["request_kind"], "other");
    let id = orch_agent_contract::observations::CANCEL_UNRECOGNIZED_APPROVAL;
    assert_eq!(request["request"]["options"][0]["id"], id);
    assert_eq!(request["request"]["options"][0]["style"], "deny");
    let reference = request["request_ref"].as_str().unwrap().to_owned();
    fake.adapter
        .resolve_approval(
            session.thread,
            &reference,
            ApprovalDecision::Option {
                option_id: id.into(),
                note: None,
            },
        )
        .await
        .unwrap();
    let rest = fake.until(|event| event.is("turn.completed")).await;
    assert_eq!(rest.last().unwrap().body["status"], "interrupted");
    assert!(said(&rest).is_empty());
}

#[tokio::test(start_paused = true)]
async fn native_hooks_settings_and_startup_observations_are_passive_and_divergence_does_not_end_work()
 {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events=fake.script(&session,&[
        "observe {\"change\":\"hook\",\"subject\":\"preToolUse\",\"value\":{\"status\":\"running\"}}",
        "observe {\"change\":\"mcp_startup\",\"subject\":\"fixture\",\"value\":{\"status\":\"failed\"}}",
        "observe {\"change\":\"approval_policy\",\"value\":\"never\",\"expected\":{\"state\":\"told\",\"value\":\"on_request\"},\"next_turn\":true}",
        "diverged automatic_compaction","diverged automatic_usage_continue","say still working",
    ]).await;
    let completed = of(&events, "item.completed");
    assert_eq!(
        completed
            .iter()
            .filter(|event| event.body["payload"]["marker"] == "harness_changed")
            .count(),
        3
    );
    assert_eq!(
        completed
            .iter()
            .filter(|event| event.body["payload"]["code"] == "harness_diverged")
            .count(),
        3
    );
    assert_eq!(
        completed
            .iter()
            .filter(|event| event.body["payload"]["code"] == "mcp_server_failed")
            .count(),
        1
    );
    assert!(
        !events
            .iter()
            .any(|event| event.is("item.started") && event.body["kind"] == "tool_call")
    );
    assert_eq!(said(&events), vec!["still working"]);
    assert_eq!(events.last().unwrap().body["status"], "completed");
}

#[tokio::test(start_paused = true)]
async fn a_reported_model_reroute_closes_the_old_request_and_reports_the_actual_new_model() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .script(
            &session,
            &["say before", "reroute reported-model", "say after"],
        )
        .await;
    let requests = model_requests(&events);
    assert!(
        requests
            .iter()
            .any(|request| request.model.as_deref() == Some("fake-large"))
    );
    assert!(
        requests
            .iter()
            .any(|request| request.model.as_deref() == Some("reported-model"))
    );
    assert!(
        of(&events, "item.completed")
            .iter()
            .any(|event| event.body["payload"]["code"] == "model_rerouted")
    );
    assert_eq!(events.last().unwrap().body["status"], "completed");
}

#[tokio::test(start_paused = true)]
async fn a_trailing_reroute_does_not_invent_a_model_request_without_following_work() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .script(&session, &["say before", "reroute reported-model"])
        .await;
    let requests = model_requests(&events);
    assert!(
        requests
            .iter()
            .all(|request| request.model.as_deref() == Some("fake-large"))
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.outcome.is_some())
            .count(),
        1
    );
    assert_eq!(events.last().unwrap().body["status"], "completed");
}

#[tokio::test(start_paused = true)]
async fn upgrade_failures_preserve_native_codes_retry_metadata_and_partial_output() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    for code in [
        "rate_limited",
        "overloaded",
        "provider_unauthorized",
        "context_overflow",
        "stream_interrupted",
        "harness_error",
    ] {
        let fault = json!({"code":code,"message":"synthetic vendor failure","retryable":code != "provider_unauthorized","retry_after_secs":17,"incomplete":code == "stream_interrupted","said":"synthetic native text"});
        let line = format!("fault {fault}");
        let events = fake.script(&session, &["say partial", line.as_str()]).await;
        let last = &events.last().unwrap().body;
        assert_eq!(last["status"], "failed");
        assert_eq!(last["error"]["code"], code);
        assert_eq!(last["error"]["retry_after_secs"], 17);
        assert_eq!(last["error"]["said"], "synthetic native text");
        assert_eq!(said(&events), vec!["partial"]);
    }
}

#[tokio::test(start_paused = true)]
async fn malformed_neutral_input_is_an_error_with_its_text_before_any_prior_tool_runs() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .script(&session, &["run must-not-run => 0", "tool {broken"])
        .await;
    assert!(
        !events
            .iter()
            .any(|event| event.is("item.started") && event.body["kind"] == "tool_call")
    );
    assert!(of(&events, "item.completed").iter().any(|event| {
        event.body["payload"]["message"]
            .as_str()
            .is_some_and(|text| text.contains("{broken"))
    }));
    assert_eq!(events.last().unwrap().body["status"], "failed");
}

#[tokio::test(start_paused = true)]
async fn native_json_controls_tolerate_added_fields_reordered_keys_and_missing_optional_values() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .script(
            &session,
            &[
                "tool {\"result\":null,\"arguments\":null,\"future_field\":{\"kept\":true},\"name\":\"future_native_tool\",\"kind\":\"other\"}",
                "fault {\"retryable\":false,\"future_field\":17,\"message\":\"future failure\",\"code\":\"new_vendor_error\"}",
            ],
        )
        .await;
    let complete = of(&events, "item.completed");
    assert!(complete.iter().any(|event| {
        event.body["payload"]["tool"]["name"] == "future_native_tool"
            && event.body["status"] == "completed"
    }));
    let last = &events.last().unwrap().body;
    assert_eq!(last["status"], "failed");
    assert_eq!(last["error"]["code"], "new_vendor_error");
    assert_eq!(last["error"]["message"], "future failure");
    assert!(last["error"]["retry_after_secs"].is_null());
}

//! Readback belongs to a real quiet native launch, not to the lazy logical-open notification.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]
mod common;

use common::corpus::{Scripts, WAIT, barrier, from_native, handshake, to_native};
use orch_adapter_claude_code::ClaudeCodeAdapter;
use orch_agent_contract::{
    adapter::{AgentAdapter, AgentStart, EventSink},
    facts::AgentProfile,
    loopback::Loopback,
    session::SessionMode,
};
use orch_plugin_contract::Secret;
use orch_testkit::harness_corpus::{Bindings, Frame, collect_through};
use orch_types::new_id;
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};

#[tokio::test]
async fn logical_open_stays_lazy_and_warm_reads_actual_control_facts_without_model_input() {
    let version = vec![
        Frame::Raw {
            line: "2.1.322 (Claude Code)".into(),
            newline: true,
        },
        Frame::Exit { code: Some(0) },
    ];
    let mut session = handshake()[..2].to_vec();
    session.push(to_native(json!({"type":"control_request","request_id":{"$replay_capture":"settings"},"request":{"subtype":"get_settings"}})));
    // This independent sparse reply must be labelled incomplete, never replaced by the launch arguments.
    session.push(from_native(json!({"type":"control_response","response":{"subtype":"success","request_id":{"$replay_ref":"settings"},"response":{"effective":{"autoContinueAtUsageLimit":false}}}})));
    session.extend(barrier());
    let clock = orch_testkit::clock::manual();
    let scripts = Scripts::new(vec![version, session], clock.clone(), Bindings::new());
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let (sink, mut events) = EventSink::channel();
    let mut adapter = ClaudeCodeAdapter::with_launcher(Arc::new(clock), scripts.clone());
    let started = adapter
        .start(
            AgentStart {
                provider_id: new_id(),
                provider_kind: None,
                agent_id: new_id(),
                harness_kind: "claude-code".into(),
                profile: AgentProfile {
                    harness_version: None,
                    attach: None,
                    config: json!({}),
                    env: BTreeMap::from([("CLAUDE_CODE_DISABLE_FEATURE_FLAGS".into(), "0".into())]),
                },
                home: Some(home.path().to_str().unwrap().into()),
                provider_config: json!({}),
                loopback: Loopback::new(1, Secret::new("offline-native-metadata")),
                binary: Some("selected-native-candidate".into()),
            },
            sink,
        )
        .await
        .unwrap();
    assert_eq!(started.harness_version, "2.1.322");
    let thread = new_id();
    let spec = common::fixture_spec(
        thread,
        new_id(),
        work.path().to_str().unwrap(),
        "offline-session",
        SessionMode::New,
        None,
    );
    adapter.open_session(spec).await.unwrap();
    assert_eq!(
        scripts.launched.lock().unwrap().len(),
        1,
        "logical open launches only the prior version probe"
    );
    assert_eq!(
        adapter.warm_session(new_id()).await.unwrap_err().code(),
        "session_not_found"
    );
    assert_eq!(
        scripts.launched.lock().unwrap().len(),
        1,
        "observation cannot manufacture a missing native session"
    );
    adapter.warm_session(thread).await.unwrap();
    let rows = collect_through(&mut events, "item.completed", WAIT)
        .await
        .unwrap();
    scripts.completed().await;
    let marker = &rows.last().unwrap().body["payload"]["data"];
    assert_eq!(marker["change"], "managed_settings_readback");
    assert_eq!(marker["value"]["status"], "incomplete");
    assert!(
        marker["value"]["missing"]
            .as_array()
            .unwrap()
            .contains(&json!("env.CLAUDE_CODE_DISABLE_FEATURE_FLAGS")),
        "an absent effective pin is not proof that it is installed"
    );
    assert!(!rows.iter().any(|row| matches!(
        row.name.as_str(),
        "turn.started" | "model.request" | "usage.updated"
    )));
    adapter.stop().await.unwrap();
    scripts.finish().await;
    let launches = scripts.launched.lock().unwrap().clone();
    assert_eq!(launches.len(), 2);
    assert!(launches.iter().all(|launch| {
        launch.program == "selected-native-candidate"
            && launch
                .env
                .get("CLAUDE_CODE_DISABLE_FEATURE_FLAGS")
                .map(String::as_str)
                == Some("1")
    }));
    let transcripts = scripts.transcripts();
    assert!(
        !transcripts
            .iter()
            .flat_map(|trace| &trace.frames)
            .any(|frame| matches!(frame,Frame::ToHarness{frame,..} if frame["type"]=="user"))
    );
}

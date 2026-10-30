//! The generic adapter against a real DeepSeek Harness (`dsh --profile acp`), with the scripted model as its only
//! provider, over Chat Completions.
//!
//! Which dsh: `ORCH_DSH_BIN` names the native executable of the runtime wheel
//! (`deepseek_harness_runtime/runtime/deepseek-harness-sdk-runtime-linux-x64` of an unpacked wheel). Without it every
//! case here is skipped, with a line that says so. The program runs on this machine, in a home of its own below the
//! test's directory, with only the variables the launch data names; its model address is the scripted server's.
//! A cold start extracts some 80 MB of the program into its cache, so the first case takes a while.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::time::Duration;

use common::{agent_start, spec_in, turn_for};
use futures::StreamExt as _;
use orch_adapter_acp::AcpAdapter;
use orch_agent_contract::adapter::{AgentAdapter, EventSink, EventStream};
use orch_agent_contract::session::{SessionMode, SessionSpec};
use orch_testkit::doubles::model::{MockConfig, MockLlm};
use orch_types::{Json, new_id};

/// How long a case waits for the next event: a cold start of the program is slow, and so is a first answer.
const WAIT: Duration = Duration::from_secs(180);

fn dsh_binary() -> Option<String> {
    match std::env::var("ORCH_DSH_BIN") {
        Ok(path) if std::path::Path::new(&path).is_file() => Some(path),
        _ => {
            eprintln!("skipped: ORCH_DSH_BIN does not name the native executable of the dsh wheel");
            None
        }
    }
}

/// The events up to and including the first of `name`, as `(name, body)`.
async fn until(events: &mut EventStream, name: &str) -> Vec<(String, Json)> {
    let mut seen: Vec<(String, Json)> = Vec::new();
    loop {
        let event = tokio::time::timeout(WAIT, events.next())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "nothing came for {WAIT:?}; so far {:?}",
                    seen.iter().map(|(name, _)| name).collect::<Vec<_>>()
                )
            })
            .expect("the adapter's stream closed");
        let (kind, body) = event.to_wire().unwrap();
        let done = kind == name;
        seen.push((kind.to_owned(), body));
        if done {
            return seen;
        }
    }
}

struct Rig {
    adapter: AcpAdapter,
    events: EventStream,
    mock: MockLlm,
    spec: SessionSpec,
    _home: tempfile::TempDir,
    _work: tempfile::TempDir,
}

async fn rig(binary: &str) -> Rig {
    rig_for_protocols(binary, &["openai-chat"]).await
}

async fn rig_for_protocols(binary: &str, protocols: &[&str]) -> Rig {
    use orch_agent_contract::arrangement::{
        self, CredentialLocation, Permission, PlaceSide, Policy, ProviderSide, Serves,
    };
    use orch_agent_contract::credentials::CredentialForm;
    use orch_agent_contract::descriptor::ArrangementType;
    let mock = MockLlm::start(MockConfig::default()).await.unwrap();
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let (provider, profile) = (new_id(), new_id());
    let mut start = agent_start(provider, profile, home.path());
    start.binary = Some(binary.to_owned());
    start.loopback = serde_json::from_value(serde_json::json!({
        "base_url": mock.url(), "token": "s".repeat(64)
    }))
    .unwrap();
    let mut adapter = AcpAdapter::new(orch_adapter_acp::programs::dsh().unwrap());
    let (sink, events) = EventSink::channel();
    let started = adapter.start(start, sink).await.unwrap();
    let provider_side = ProviderSide {
        serves: protocols
            .iter()
            .map(|protocol| Serves::Forwarded {
                protocol: (*protocol).to_owned(),
            })
            .collect(),
        issues: vec![CredentialForm::api_key()],
        credential: CredentialLocation::AtHome,
    };
    let place = PlaceSide {
        environment_id: new_id(),
        offers: Vec::new(),
        executors: Vec::new(),
        can_run_harness: true,
        same_machine: false,
        harness_version: None,
    };
    let policy = Policy {
        permission: Permission {
            provider: ArrangementType::ALL.to_vec(),
            ..Permission::default()
        },
        ..Policy::default()
    };
    let chosen = arrangement::choose(
        &started.descriptor,
        &provider_side,
        &place,
        &policy,
        Some(ArrangementType::KeySeparated),
    )
    .unwrap();
    let mut spec = spec_in(work.path(), profile, provider, SessionMode::New);
    spec.provider = orch_agent_contract::session::ProviderAccess::Forwarded {
        provider_id: provider,
        protocol: chosen.protocol.unwrap(),
    };
    spec.settings.model = "scripted".to_owned();
    spec.settings.available_models = vec!["scripted".to_owned()];
    spec.settings.orch_tools = Vec::new();
    Rig {
        adapter,
        events,
        mock,
        spec,
        _home: home,
        _work: work,
    }
}

fn final_text(seen: &[(String, Json)]) -> String {
    seen.iter()
        .filter(|(kind, body)| kind == "item.completed" && body["payload"]["phase"] == "final")
        .map(|(_, body)| {
            body["payload"]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .next_back()
        .expect("a finished message")
}

async fn protocol_turn(binary: &str, protocols: &[&str], expected_path: &str) {
    let mut rig = rig_for_protocols(binary, protocols).await;
    rig.mock
        .set_default("say Hello from this protocol.\n")
        .unwrap();
    let opened = rig.adapter.open_session(rig.spec.clone()).await.unwrap();
    assert!(!opened.native_session_ref.is_empty());
    rig.adapter
        .start_turn(turn_for(&rig.spec, "hello"))
        .await
        .unwrap();
    let seen = until(&mut rig.events, "turn.completed").await;
    assert_eq!(seen.last().unwrap().1["status"], "completed", "{seen:#?}");
    assert_eq!(final_text(&seen), "Hello from this protocol.");
    let requests = rig.mock.settled(1).await;
    assert!(!requests.is_empty());
    assert!(
        requests
            .iter()
            .all(|request| request.path.ends_with(expected_path)),
        "{requests:#?}"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_dsh_thread_opens_and_completes_on_a_responses_only_provider() {
    let Some(binary) = dsh_binary() else { return };
    protocol_turn(&binary, &["openai-responses"], "/responses").await;
}

#[tokio::test]
async fn a_dsh_thread_prefers_chat_when_its_provider_serves_both_protocols() {
    let Some(binary) = dsh_binary() else { return };
    protocol_turn(
        &binary,
        &["openai-responses", "openai-chat"],
        "/chat/completions",
    )
    .await;
}

#[tokio::test]
async fn a_dsh_thread_gives_a_finished_message_and_only_model_requests_leave_without_what_its_vendor_is_sent()
 {
    let Some(binary) = dsh_binary() else { return };
    let mut rig = rig(&binary).await;
    // The program puts its own context after the person's words, so the model answers from a script it was set.
    rig.mock
        .set_default("say Hello from the scripted model.\n")
        .unwrap();
    let opened = rig.adapter.open_session(rig.spec.clone()).await.unwrap();
    assert!(!opened.native_session_ref.is_empty());
    let turn = turn_for(&rig.spec, "hello dsh");
    rig.adapter.start_turn(turn).await.unwrap();
    let seen = until(&mut rig.events, "turn.completed").await;
    assert_eq!(seen.last().unwrap().1["status"], "completed", "{seen:#?}");
    assert_eq!(final_text(&seen), "Hello from the scripted model.");
    let request = seen
        .iter()
        .find(|(kind, _)| kind == "model.request")
        .expect("a request of the turn");
    assert_eq!(request.1["request"]["outcome"]["status"], "done");
    // What left: model requests only, to the scripted model's Chat Completions route, with nothing its vendor is sent.
    let requests = rig.mock.settled(1).await;
    assert!(!requests.is_empty());
    for recorded in &requests {
        assert!(
            recorded.path.ends_with("/chat/completions") || recorded.path.ends_with("/models"),
            "only model requests leave: {}",
            recorded.path
        );
        assert!(
            recorded
                .headers
                .keys()
                .all(|name| !name.starts_with("x-deepseek-harness")),
            "a header that names the client left: {:?}",
            recorded.headers.keys().collect::<Vec<_>>()
        );
        let body = recorded.body.to_string();
        for member in ["dsh_session_log", "dsh_plugin_packages"] {
            assert!(!body.contains(member), "{member} left");
        }
        assert_eq!(
            recorded
                .header("authorization")
                .map(|value| value.starts_with("Bearer ")),
            Some(true)
        );
    }
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_dsh_thread_is_opened_again_with_what_was_said_before() {
    let Some(binary) = dsh_binary() else { return };
    let mut rig = rig(&binary).await;
    let opened = rig.adapter.open_session(rig.spec.clone()).await.unwrap();
    rig.adapter
        .start_turn(turn_for(&rig.spec, "remember the word quince"))
        .await
        .unwrap();
    until(&mut rig.events, "turn.completed").await;
    rig.adapter
        .close_session(rig.spec.settings.thread_id)
        .await
        .unwrap();
    let mut again = rig.spec.clone();
    again.settings.mode = SessionMode::Resume;
    again.settings.native_session_ref = Some(opened.native_session_ref.clone());
    let resumed = rig.adapter.open_session(again).await.unwrap();
    assert!(resumed.resumed);
    assert_eq!(resumed.native_session_ref, opened.native_session_ref);
    rig.adapter
        .start_turn(turn_for(&rig.spec, "what was the word?"))
        .await
        .unwrap();
    until(&mut rig.events, "turn.completed").await;
    let requests = rig.mock.settled(2).await;
    let last = requests
        .iter()
        .rfind(|recorded| recorded.path.ends_with("/chat/completions"))
        .expect("a request of the second turn");
    assert!(
        last.body.to_string().contains("quince"),
        "the request of the second turn holds the first: {}",
        last.body
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_dsh_turn_that_is_stopped_ends_as_stopped() {
    let Some(binary) = dsh_binary() else { return };
    let mut rig = rig(&binary).await;
    rig.adapter.open_session(rig.spec.clone()).await.unwrap();
    // The scripted model writes slowly: a few characters every 400 ms.
    rig.mock
        .set_default("chunk 3\ndelay 400\nsay Once upon a time there was a very long sentence that goes on and on and on.\n")
        .unwrap();
    let turn = turn_for(&rig.spec, "write at length");
    let turn_id = turn.turn_id;
    rig.adapter.start_turn(turn).await.unwrap();
    // Wait until the model request is under way, then stop the turn.
    let deadline = std::time::Instant::now() + WAIT;
    while rig.mock.requests().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the program never asked the model"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    rig.adapter
        .interrupt(rig.spec.settings.thread_id, turn_id)
        .await
        .unwrap();
    let seen = until(&mut rig.events, "turn.completed").await;
    assert_eq!(seen.last().unwrap().1["status"], "interrupted", "{seen:#?}");
    rig.adapter.stop().await.unwrap();
}

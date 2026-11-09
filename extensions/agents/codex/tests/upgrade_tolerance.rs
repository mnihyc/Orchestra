//! Compatibility observations run through the production native reader and approval actor, with raw logging off.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]
mod common;

use common::fake::FakeLauncher;
use futures::StreamExt;
use orch_adapter_codex::CodexAdapter;
use orch_agent_contract::adapter::{AgentAdapter, AgentStart, EventSink, EventStream};
use orch_agent_contract::descriptor::{ApprovalMode, SandboxMode};
use orch_agent_contract::facts::AgentProfile;
use orch_agent_contract::loopback::Loopback;
use orch_agent_contract::observations::CANCEL_UNRECOGNIZED_APPROVAL;
use orch_agent_contract::session::{ApprovalDecision, SessionMode, TurnStart};
use orch_plugin_contract::Secret;
use orch_testkit::harness_corpus::{Observation, assert_unrecognized};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(10);
struct Rig {
    adapter: CodexAdapter,
    fake: Arc<FakeLauncher>,
    events: EventStream,
    _home: tempfile::TempDir,
    agent: Id,
    provider: Id,
}
impl Rig {
    async fn new() -> Self {
        Self::with_configuration(json!({})).await
    }
    async fn with_configuration(config: Json) -> Self {
        let count = AtomicUsize::new(0);
        let fake = Arc::new(FakeLauncher::new(move |method, _| {
            Ok(match method {
                "config/read" => json!({"config":config,"origins":{}}),
                "environment/info" => {
                    serde_json::from_str(include_str!("fixtures/environment-info-0.162.1.json"))
                        .unwrap()
                }
                "thread/start" => {
                    json!({"thread":{"id":format!("fixture-thread-{}", count.fetch_add(1, Ordering::SeqCst)),"path":null}})
                }
                "turn/start" => json!({"turn":{"id":"fixture-turn","status":"inProgress"}}),
                _ => json!({}),
            })
        }));
        let mut adapter = CodexAdapter::with_launcher(fake.clone())
            .with_clock(Arc::new(orch_testkit::clock::manual()));
        let home = tempfile::tempdir().unwrap();
        let (sink, events) = EventSink::channel();
        let (agent, provider) = (new_id(), new_id());
        adapter
            .start(
                AgentStart {
                    provider_kind: None,
                    provider_id: provider,
                    agent_id: agent,
                    harness_kind: "codex".into(),
                    profile: AgentProfile::default(),
                    home: Some(home.path().to_string_lossy().into_owned()),
                    provider_config: json!({}),
                    loopback: Loopback::new(41999, Secret::new("fixture-loopback")),
                    binary: None,
                },
                sink,
            )
            .await
            .unwrap();
        Self {
            adapter,
            fake,
            events,
            _home: home,
            agent,
            provider,
        }
    }
    async fn through(&mut self, name: &str, count: usize) -> Vec<Observation> {
        let mut rows = vec![];
        let mut seen = 0;
        while seen < count {
            let event = tokio::time::timeout(WAIT, self.events.next())
                .await
                .unwrap()
                .unwrap();
            let event = Observation::from_event(&event).unwrap();
            assert_ne!(
                event.name, "raw.frame",
                "compatibility observations do not depend on raw logging"
            );
            if event.name == name {
                seen += 1;
            }
            rows.push(event);
        }
        rows
    }
    async fn open(&mut self) -> (Id, String) {
        let thread = new_id();
        let mut spec = common::plain_spec(
            "/fixture/work",
            self.agent,
            self.provider,
            new_id(),
            thread,
            SessionMode::New,
            None,
        );
        spec.settings.approval_mode = ApprovalMode::Bypass;
        assert!(!spec.settings.raw_events);
        let opened = self.adapter.open_session(spec).await.unwrap();
        self.through("session.opened", 1).await;
        (thread, opened.native_session_ref)
    }
    async fn turn(&mut self, thread: Id, native: &str) -> Id {
        let turn = new_id();
        self.adapter
            .start_turn(TurnStart {
                native_model: None,
                native_wake: None,
                plan: false,
                thread_id: thread,
                turn_id: turn,
                text: "Fixture input".into(),
                images: vec![],
                model: common::MODEL.into(),
                effort: None,
                params: json!({}),
                approval_mode: ApprovalMode::Bypass,
                sandbox_mode: SandboxMode::Open,
            })
            .await
            .unwrap();
        self.fake.state.notify(
            "turn/started",
            json!({"threadId":native,"turn":{"id":"fixture-turn","status":"inProgress"}}),
        );
        self.through("turn.started", 1).await;
        turn
    }
    async fn answer(&self, id: i64) -> Json {
        tokio::time::timeout(WAIT, async {
            loop {
                if let Some(answer) = self.fake.state.answer_to(id) {
                    return answer;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn unknown_events_are_counted_while_idle_without_retaining_private_bodies_or_foreign_frames()
{
    let mut rig = Rig::new().await;
    let (_, native) = rig.open().await;
    rig.fake.state.notify(
        "future/update",
        json!({"threadId":"another-private-session","secret":"foreign payload"}),
    );
    for _ in 0..2 {
        rig.fake.state.notify(
            "future/update",
            json!({"threadId":native,"secret":"private payload"}),
        );
    }
    let rows = rig.through("item.completed", 2).await;
    assert_unrecognized(&rows, "Codex", "future/update", 2).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .all(|row| row.body["payload"]["data"]["turn_id"].is_null())
    );
    let encoded = serde_json::to_string(&rows).unwrap();
    assert!(!encoded.contains("private payload") && !encoded.contains("foreign payload"));
    assert_ne!(rows[0].body["native_ref"], rows[1].body["native_ref"]);
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn an_unknown_event_needs_its_own_native_turn_correlation() {
    let mut rig = Rig::new().await;
    let (thread, native) = rig.open().await;
    let turn = rig.turn(thread, &native).await;
    rig.fake.state.notify(
        "future/update",
        json!({"threadId":native,"turnId":"a-stale-native-turn"}),
    );
    rig.fake.state.notify(
        "future/update",
        json!({"threadId":native,"turnId":"fixture-turn"}),
    );
    let rows = rig.through("item.completed", 2).await;
    assert_unrecognized(&rows, "Codex", "future/update", 2).unwrap();
    assert_eq!(
        rows.iter().filter(|row| row.name == "item.started").count(),
        1
    );
    let completed: Vec<_> = rows
        .iter()
        .filter(|row| row.name == "item.completed")
        .collect();
    assert_eq!(completed.len(), 2);
    assert!(completed[0].body["payload"]["data"]["turn_id"].is_null());
    assert_eq!(completed[1].body["payload"]["data"]["turn_id"], json!(turn));
    let started = rows.iter().find(|row| row.name == "item.started").unwrap();
    assert_eq!(started.body["turn_id"], json!(turn));
    assert_eq!(started.body["native_ref"], completed[1].body["native_ref"]);
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn unknown_approval_kinds_and_methods_have_only_real_cancellation_even_in_bypass() {
    for (method, kind, protocol_error) in [
        (
            "item/commandExecution/requestApproval",
            "futureCommand",
            false,
        ),
        ("item/futureTool/requestApproval", "futurePermission", true),
    ] {
        let mut rig = Rig::new().await;
        let (thread, native) = rig.open().await;
        rig.turn(thread, &native).await;
        rig.fake.state.ask(700,method,json!({"threadId":native,"turnId":"fixture-turn","itemId":"future-item","kind":kind,"credential":"private input"}));
        let rows = rig.through("approval.requested", 1).await;
        let asked = rows.last().unwrap();
        let request = &asked.body["request"];
        assert_eq!(request["request_kind"], "other");
        let options = request["options"].as_array().unwrap();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0]["id"], CANCEL_UNRECOGNIZED_APPROVAL);
        assert_eq!(options[0]["style"], "deny");
        assert!(
            !serde_json::to_string(request)
                .unwrap()
                .contains("private input")
        );
        assert!(
            rig.fake.state.answer_to(700).is_none(),
            "an unknown approval cannot be bypassed"
        );
        let reference = asked.body["request_ref"].as_str().unwrap();
        assert!(
            rig.adapter
                .resolve_approval(
                    thread,
                    reference,
                    ApprovalDecision::Option {
                        option_id: "once".into(),
                        note: None
                    }
                )
                .await
                .is_err()
        );
        rig.adapter
            .resolve_approval(
                thread,
                reference,
                ApprovalDecision::Option {
                    option_id: CANCEL_UNRECOGNIZED_APPROVAL.into(),
                    note: None,
                },
            )
            .await
            .unwrap();
        let answer = rig.answer(700).await;
        if protocol_error {
            assert_eq!(answer["error"]["code"], -32601);
            assert!(answer.get("result").is_none());
        } else {
            assert_eq!(answer["result"], json!({"decision":"cancel"}));
        }
        rig.adapter.stop().await.unwrap();
    }
}

#[tokio::test]
async fn an_unknown_nonapproval_request_is_counted_and_rejected_without_waiting() {
    let mut rig = Rig::new().await;
    let (_, native) = rig.open().await;
    rig.fake
        .state
        .ask(701, "future/request", json!({"threadId":native}));
    let rows = rig.through("item.completed", 1).await;
    assert_unrecognized(&rows, "Codex", "future/request", 1).unwrap();
    assert_eq!(rig.answer(701).await["error"]["code"], -32601);
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn malformed_native_text_is_an_error_item_and_the_next_frame_is_still_read() {
    let mut rig = Rig::new().await;
    let (_, native) = rig.open().await;
    rig.fake.state.say_raw("{broken fixture\n");
    let rows = rig.through("item.completed", 1).await;
    assert_eq!(
        rows.last().unwrap().body["payload"]["code"],
        "malformed_native_frame"
    );
    assert!(
        rows.last().unwrap().body["payload"]["message"]
            .as_str()
            .unwrap()
            .contains("{broken fixture")
    );
    assert!(!rows.iter().any(|row| row.name == "turn.started"));
    rig.fake
        .state
        .notify("future/afterMalformed", json!({"threadId":native}));
    assert_unrecognized(
        &rig.through("item.completed", 1).await,
        "Codex",
        "future/afterMalformed",
        1,
    )
    .unwrap();
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn ambiguous_process_text_is_never_copied_between_sessions() {
    let mut rig = Rig::new().await;
    let (first, _) = rig.open().await;
    let (second, _) = rig.open().await;
    rig.fake.state.say_raw("{private malformed payload\n");
    let rows = rig.through("item.completed", 2).await;
    assert_eq!(rows.len(), 2);
    let ids = rows
        .iter()
        .map(|row| row.body["thread_id"].clone())
        .collect::<Vec<_>>();
    assert!(ids.contains(&json!(first)) && ids.contains(&json!(second)));
    for row in &rows {
        assert_eq!(row.body["payload"]["code"], "malformed_native_frame");
        assert!(
            row.body["payload"]["message"]
                .as_str()
                .unwrap()
                .contains("several sessions")
        );
        assert!(
            !serde_json::to_string(row)
                .unwrap()
                .contains("private malformed payload")
        );
    }
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn every_managed_pin_reported_enabled_is_divergence_without_native_work_or_private_config() {
    for (pin, expected) in orch_adapter_codex::process::automation_pins() {
        let actual = match expected {
            Json::Bool(_) => json!(true),
            Json::Number(_) => json!(50),
            Json::Array(_) => json!(["private notification command"]),
            _ => panic!("the native pin fixture must cover the exact production type"),
        };
        let mut config = json!({"model_provider":null});
        if let Some((section, member)) = pin.split_once('.') {
            config[section] = json!({(member):actual});
        } else {
            config[&pin] = actual;
        }
        let mut rig = Rig::with_configuration(config).await;
        let (thread, _) = rig.open().await;
        let rows = rig.through("item.completed", 2).await;
        assert_eq!(rows.len(), 2, "{pin}: {rows:?}");
        assert_eq!(rows[0].body["payload"]["marker"], "harness_changed");
        assert_eq!(
            rows[0].body["payload"]["data"]["change"],
            "managed_settings_readback"
        );
        assert_eq!(
            rows[0].body["payload"]["data"]["value"]["pins"],
            json!([pin])
        );
        assert_eq!(rows[0].body["payload"]["data"]["diverged"], true);
        assert!(rows[0].body["payload"]["data"]["turn_id"].is_null());
        assert_eq!(rows[1].body["payload"]["code"], "harness_diverged");
        assert_eq!(rows[1].body["thread_id"], json!(thread));
        assert!(
            !serde_json::to_string(&rows)
                .unwrap()
                .contains("private notification command")
        );
        assert!(
            rig.fake.state.calls("turn/start").is_empty(),
            "observing {pin} cannot issue a turn"
        );
        assert!(
            rig.fake.state.calls("command/exec").is_empty(),
            "observing {pin} cannot execute a command"
        );
        rig.adapter.stop().await.unwrap();
    }
}

#[tokio::test]
async fn absent_and_matching_effective_pins_do_not_claim_a_launch_violation() {
    let mut config = json!({});
    for (pin, value) in orch_adapter_codex::process::automation_pins() {
        if let Some((section, member)) = pin.split_once('.') {
            if !config[section].is_object() {
                config[section] = json!({});
            }
            config[section][member] = value;
        } else {
            config[&pin] = value;
        }
    }
    for config in [json!({}), config] {
        let mut rig = Rig::with_configuration(config).await;
        let (_, native) = rig.open().await;
        rig.fake
            .state
            .notify("future/barrier", json!({"threadId":native}));
        let rows = rig.through("item.completed", 1).await;
        assert_eq!(rows.len(), 1);
        assert_unrecognized(&rows, "Codex", "future/barrier", 1).unwrap();
        rig.adapter.stop().await.unwrap();
    }
}

#[tokio::test]
async fn pinned_terminal_end_is_unavailable_without_probing_and_interrupt_close_still_work() {
    use orch_agent_contract::background::BackgroundEndState;
    let mut rig = Rig::new().await;
    let (thread, native) = rig.open().await;
    let turn = rig.turn(thread, &native).await;
    rig.fake.state.notify("item/started", json!({"threadId":native,"turnId":"fixture-turn","startedAtMs":0,"item":{
        "type":"commandExecution","id":"retained-command","command":"fixture command","cwd":"/fixture/work","commandActions":[],"status":"inProgress","processId":"fixture-process"}}));
    rig.through("item.started", 1).await;
    let background = rig.adapter.background_commands(thread).await.unwrap();
    assert!(!background.supported);
    assert_eq!(
        background.reason.as_deref(),
        Some("This Codex version cannot end background terminals.")
    );
    let end = rig
        .adapter
        .end_background_command(thread, "retained-command")
        .await
        .unwrap();
    assert_eq!(end.state, BackgroundEndState::Unknown);
    assert_eq!(end.reason, background.reason);
    rig.adapter.interrupt(thread, turn).await.unwrap();
    rig.fake.state.notify("turn/completed",json!({"threadId":native,"turn":{"id":"fixture-turn","status":"interrupted","items":[],"error":null}}));
    let rows = rig.through("turn.command_cleanup", 1).await;
    assert_eq!(rows.last().unwrap().body["result"]["unknown"], 1);
    assert!(!rows.iter().any(|row|row.name=="item.completed"&&row.body["native_ref"]=="retained-command"),"an interrupt acknowledgement is no process-end proof");
    rig.adapter.close_session(thread).await.unwrap();
    assert_eq!(rig.fake.state.calls("turn/interrupt").len(), 1);
    assert_eq!(rig.fake.state.calls("thread/unsubscribe").len(), 1);
    assert!(
        rig.fake
            .state
            .calls("thread/backgroundTerminals/list")
            .is_empty()
    );
    assert!(
        rig.fake
            .state
            .calls("thread/backgroundTerminals/terminate")
            .is_empty()
    );
    rig.adapter.stop().await.unwrap();
}

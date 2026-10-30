//! What the real harness cannot be made to do here, against a harness played in the test: a version the
//! adapter does not drive, a start that fails, a request for more permissions, a question, an MCP server's
//! request for input, and a sign-in by a code, which needs the vendor's servers.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use common::fake::FakeLauncher;
use common::{kinds, plain_spec, session_events};
use futures::StreamExt;
use orch_adapter_codex::CodexAdapter;
use orch_agent_contract::adapter::{
    AdapterError, AdapterEvent, AgentAdapter, AgentStart, EventSink, EventStream,
};
use orch_agent_contract::descriptor::{ApprovalMode, HarnessSupport};
use orch_agent_contract::events::{
    HarnessRequestDetail, HarnessRequestKind, ItemContent, SessionEvent, TurnOutcome,
};
use orch_agent_contract::facts::{AgentProfile, HarnessState};
use orch_agent_contract::history::ExportKind;
use orch_agent_contract::login::{LoginRequest, LoginStateKind, ShowKind};
use orch_agent_contract::loopback::Loopback;
use orch_agent_contract::session::{
    ApprovalDecision, ConfigTrust, ElicitationAction, SessionMode, TurnStart,
    WorkspaceConfiguration,
};
use orch_plugin_contract::Secret;
use orch_types::{Id, Json, new_id};
use serde_json::json;

struct Setup {
    adapter: CodexAdapter,
    events: EventStream,
    sink: EventSink,
    fake: Arc<FakeLauncher>,
    home: tempfile::TempDir,
}

fn setup(fake: FakeLauncher) -> Setup {
    let fake = Arc::new(fake);
    let adapter = CodexAdapter::with_launcher(
        Arc::clone(&fake) as Arc<dyn orch_adapter_codex::process::Launcher>
    );
    let (sink, events) = EventSink::channel();
    Setup {
        adapter,
        events,
        sink,
        fake,
        home: tempfile::tempdir().unwrap(),
    }
}

impl Setup {
    fn start_request(&self) -> AgentStart {
        AgentStart {
            provider_id: new_id(),
            agent_id: new_id(),
            harness_kind: "codex".into(),
            profile: AgentProfile::default(),
            home: Some(self.home.path().to_string_lossy().into_owned()),
            provider_config: json!({}),
            loopback: Loopback::new(41999, Secret::new("secret-token")),
            binary: None,
        }
    }

    async fn next(&mut self) -> AdapterEvent {
        tokio::time::timeout(Duration::from_secs(30), self.events.next())
            .await
            .expect("an event")
            .expect("open")
    }

    async fn until(&mut self, done: impl Fn(&AdapterEvent) -> bool) -> Vec<AdapterEvent> {
        let mut taken = Vec::new();
        loop {
            let event = self.next().await;
            let finished = done(&event);
            taken.push(event);
            if finished {
                return taken;
            }
        }
    }

    fn drain(&mut self) -> Vec<AdapterEvent> {
        let mut taken = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            taken.push(event);
        }
        taken
    }
}

#[tokio::test]
async fn resuming_or_adopting_does_not_report_a_successful_first_turn_as_an_error() {
    for mode in [SessionMode::Resume, SessionMode::Adopt] {
        for changed_instructions in [false, true] {
            let mut setup = setup(working());
            setup
                .adapter
                .start(setup.start_request(), setup.sink.clone())
                .await
                .unwrap();
            let thread = new_id();
            let mut spec = plain_spec(
                "/workspace/web",
                new_id(),
                new_id(),
                new_id(),
                thread,
                mode,
                Some("thr_1"),
            );
            if changed_instructions {
                spec.settings.instructions =
                    "Changed standing instructions for the next new session.".into();
            }
            let opened = setup.adapter.open_session(spec).await.unwrap();
            assert!(opened.resumed);
            assert_eq!(opened.native_session_ref, "thr_1");
            assert!(setup.fake.state.calls("turn/start").is_empty());
            let turn = TurnStart {
                plan: false,
                thread_id: thread,
                turn_id: new_id(),
                text: "Continue from the carried history.".into(),
                images: Vec::new(),
                model: "mock-large".into(),
                effort: Some("high".into()),
                params: json!({}),
                approval_mode: ApprovalMode::Ask,
                sandbox_mode: orch_agent_contract::descriptor::SandboxMode::Workspace,
            };
            setup.adapter.start_turn(turn.clone()).await.unwrap();
            setup.fake.state.notify(
                "turn/started",
                json!({ "threadId": "thr_1", "turn": { "id": "turn_1", "status": "inProgress" } }),
            );
            setup.fake.state.notify(
                "turn/completed",
                json!({ "threadId": "thr_1", "turn": { "id": "turn_1", "status": "completed", "error": null } }),
            );
            let events = setup
                .until(|event| {
                    matches!(
                        event,
                        AdapterEvent::Session {
                            event: SessionEvent::TurnCompleted(_),
                            ..
                        }
                    )
                })
                .await;
            assert!(
                !events.iter().any(|event| matches!(event,
                    AdapterEvent::Session { event: SessionEvent::ItemStarted(item), .. }
                        if matches!(item.content, ItemContent::Error(_))
                )),
                "a successful resumed turn must not fabricate an error: {mode:?}, changed instructions {changed_instructions}"
            );
            assert!(matches!(events.last(), Some(AdapterEvent::Session {
                event: SessionEvent::TurnCompleted(end), ..
            }) if end.turn_id == turn.turn_id && end.status == TurnOutcome::Completed));
            let started = setup.fake.state.calls("turn/start");
            assert_eq!(started.len(), 1, "resuming does not replay model input");
            assert!(started[0]["params"].to_string().contains(&turn.text));
            setup.adapter.stop().await.unwrap();
        }
    }
}

#[tokio::test]
async fn a_plan_turn_plans_read_only_and_the_turn_after_it_goes_back_to_the_default_mode() {
    let mut setup = setup(working());
    let (thread, first) = running_turn(&mut setup).await;
    let finish = |setup: &mut Setup, turn: &str| {
        setup.fake.state.notify(
            "turn/completed",
            json!({ "threadId": "thr_1", "turn": { "id": turn, "status": "completed", "error": null } }),
        );
    };
    finish(&mut setup, "turn_1");
    setup
        .until(|event| {
            matches!(
                event,
                AdapterEvent::Session {
                    event: SessionEvent::TurnCompleted(_),
                    ..
                }
            )
        })
        .await;
    // An ordinary turn states no collaboration mode at all.
    assert!(
        setup.fake.state.calls("turn/start")[0]["params"]
            .get("collaborationMode")
            .is_none()
    );
    for (n, plan) in [(1, true), (2, false), (3, false)] {
        let turn = TurnStart {
            plan,
            turn_id: new_id(),
            ..first.clone()
        };
        setup.adapter.start_turn(turn).await.unwrap();
        let id = format!("turn_{}", n + 1);
        setup.fake.state.notify(
            "turn/started",
            json!({ "threadId": "thr_1", "turn": { "id": id, "status": "inProgress" } }),
        );
        finish(&mut setup, &id);
        setup
            .until(|event| {
                matches!(
                    event,
                    AdapterEvent::Session {
                        event: SessionEvent::TurnCompleted(_),
                        ..
                    }
                )
            })
            .await;
    }
    let started = setup.fake.state.calls("turn/start");
    let planned = &started[1]["params"];
    assert_eq!(planned["collaborationMode"]["mode"], "plan");
    assert_eq!(
        planned["collaborationMode"]["settings"]["model"],
        "mock-large"
    );
    assert_eq!(
        planned["collaborationMode"]["settings"]["reasoning_effort"],
        "high"
    );
    assert_eq!(
        planned["sandboxPolicy"]["type"], "readOnly",
        "a plan changes nothing"
    );
    assert_eq!(
        planned["approvalPolicy"], "untrusted",
        "the approval mode stays as it is"
    );
    let after = &started[2]["params"];
    assert_eq!(after["collaborationMode"]["mode"], "default");
    assert_eq!(after["sandboxPolicy"]["type"], "workspaceWrite");
    assert!(started[3]["params"].get("collaborationMode").is_none());
    let _ = thread;
    setup.adapter.stop().await.unwrap();
}

/// The shell reported by the pinned 0.162.1 exec server after its native connection succeeds.
fn executor_info() -> Json {
    serde_json::from_str(include_str!("fixtures/environment-info-0.162.1.json")).unwrap()
}

/// A fake that opens threads and starts turns.
fn working() -> FakeLauncher {
    FakeLauncher::new(|method, _| match method {
        "environment/info" => Ok(executor_info()),
        "thread/start" | "thread/resume" => {
            Ok(json!({ "thread": { "id": "thr_1", "path": null } }))
        }
        "turn/start" => Ok(json!({ "turn": { "id": "turn_1", "status": "inProgress" } })),
        _ => Ok(json!({})),
    })
}

#[tokio::test]
async fn a_resumed_session_explains_pinned_instructions_once_without_reporting_a_failed_item() {
    use orch_agent_contract::events::{ItemContent, ItemStatus, MarkerPayload, TurnOutcome};
    use std::sync::atomic::AtomicUsize;

    let starts = AtomicUsize::new(0);
    let mut setup = setup(FakeLauncher::new(move |method, _| match method {
        "environment/info" => Ok(executor_info()),
        "thread/resume" => Ok(json!({ "thread": { "id": "thr_1", "path": null } })),
        "turn/start" => {
            let ordinal = starts.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(json!({ "turn": { "id": format!("turn_{ordinal}"), "status": "inProgress" } }))
        }
        _ => Ok(json!({})),
    }));
    setup
        .adapter
        .start(setup.start_request(), setup.sink.clone())
        .await
        .unwrap();
    let thread = new_id();
    let spec = plain_spec(
        "/workspace/web",
        new_id(),
        new_id(),
        new_id(),
        thread,
        SessionMode::Resume,
        Some("thr_1"),
    );
    setup.adapter.open_session(spec).await.unwrap();
    let mut events = setup.drain();
    for ordinal in 1..=2 {
        let turn = new_id();
        setup
            .adapter
            .start_turn(TurnStart {
                plan: false,
                thread_id: thread,
                turn_id: turn,
                text: "Continue the work".into(),
                images: Vec::new(),
                model: "mock-large".into(),
                effort: Some("high".into()),
                params: json!({}),
                approval_mode: ApprovalMode::Ask,
                sandbox_mode: orch_agent_contract::descriptor::SandboxMode::Workspace,
            })
            .await
            .unwrap();
        let native = format!("turn_{ordinal}");
        setup.fake.state.notify(
            "turn/started",
            json!({ "threadId": "thr_1", "turn": { "id": native, "status": "inProgress" } }),
        );
        setup.fake.state.notify(
            "turn/completed",
            json!({ "threadId": "thr_1", "turn": { "id": native, "status": "completed", "error": null } }),
        );
        events.extend(setup.until(|event| matches!(event,
            AdapterEvent::Session { event: SessionEvent::TurnCompleted(done), .. }
                if done.turn_id == turn && done.status == TurnOutcome::Completed && done.error.is_none()
        )).await);
    }
    let mut notices = Vec::new();
    for event in &events {
        if let AdapterEvent::Session { event, .. } = event {
            match event {
                SessionEvent::ItemStarted(item) => {
                    assert!(!matches!(item.content, ItemContent::Error(_)), "{item:?}");
                    if let ItemContent::Marker(MarkerPayload::InstructionsDeferred { program }) =
                        &item.content
                    {
                        assert_eq!(program, "Codex");
                        notices.push(item.native_ref.clone());
                    }
                }
                SessionEvent::ItemCompleted(item) => assert_eq!(item.status, ItemStatus::Completed),
                _ => {}
            }
        }
    }
    assert_eq!(notices.len(), 1, "one notice per resumed session");
    assert!(events.iter().any(|event| matches!(event,
        AdapterEvent::Session { event: SessionEvent::ItemCompleted(item), .. }
            if item.native_ref == notices[0] && item.payload == json!({ "marker": "instructions_deferred", "data": { "program": "Codex" } })
    )));
    assert_eq!(setup.fake.state.calls("thread/resume").len(), 1);
    assert_eq!(setup.fake.state.calls("turn/start").len(), 2);
    assert!(setup.fake.state.calls("thread/inject_items").is_empty());
    setup.adapter.stop().await.unwrap();
}

async fn running_turn(setup: &mut Setup) -> (Id, TurnStart) {
    running_turn_in_mode(setup, ApprovalMode::Ask, false).await
}

async fn running_turn_in_mode(
    setup: &mut Setup,
    approval_mode: ApprovalMode,
    plan: bool,
) -> (Id, TurnStart) {
    let start = setup.start_request();
    setup
        .adapter
        .start(start, setup.sink.clone())
        .await
        .unwrap();
    let thread = new_id();
    let spec = plain_spec(
        "/workspace/web",
        new_id(),
        new_id(),
        new_id(),
        thread,
        SessionMode::New,
        None,
    );
    setup.adapter.open_session(spec).await.unwrap();
    let turn = TurnStart {
        plan,
        thread_id: thread,
        turn_id: new_id(),
        text: "go".into(),
        images: Vec::new(),
        model: "mock-large".into(),
        effort: Some("high".into()),
        params: json!({}),
        approval_mode,
        sandbox_mode: orch_agent_contract::descriptor::SandboxMode::Workspace,
    };
    setup.adapter.start_turn(turn.clone()).await.unwrap();
    setup.fake.state.notify(
        "turn/started",
        json!({ "threadId": "thr_1", "turn": { "id": "turn_1", "status": "inProgress" } }),
    );
    setup
        .until(|event| {
            matches!(
                event,
                AdapterEvent::Session {
                    event: SessionEvent::TurnStarted(_),
                    ..
                }
            )
        })
        .await;
    (thread, turn)
}

#[tokio::test]
async fn no_thread_is_opened_with_a_history_mode_of_orchs_choosing() {
    // The vendor's own choice of how a thread's history is kept is the only one: it is never forced for a managed
    // home, on any build, for a new thread or an existing one.
    for version in ["0.162.1", "0.165.0", "0.166.0"] {
        for (personal, mode) in [
            (false, SessionMode::New),
            (true, SessionMode::New),
            (false, SessionMode::Resume),
        ] {
            let mut setup = setup(working());
            setup.fake.state.set_version(version);
            let mut start = setup.start_request();
            if personal {
                start.home = None;
            }
            let started = setup.adapter.start(start, setup.sink.clone()).await;
            if version == "0.162.1" {
                // Older builds cannot open a thread at all; changing the history mode must not bypass that boundary.
                let error = started.unwrap_err();
                assert_eq!(error.code(), "harness_unavailable");
                assert!(setup.fake.state.calls("thread/start").is_empty());
                assert!(setup.fake.state.calls("thread/resume").is_empty());
                continue;
            }
            started.unwrap();
            let spec = plain_spec(
                "/workspace/web",
                new_id(),
                new_id(),
                new_id(),
                new_id(),
                mode,
                (mode == SessionMode::Resume).then_some("previous-thread"),
            );
            setup.adapter.open_session(spec).await.unwrap();
            let method = if mode == SessionMode::New {
                "thread/start"
            } else {
                "thread/resume"
            };
            assert!(
                setup.fake.state.calls(method)[0]["params"]
                    .get("historyMode")
                    .is_none(),
                "{version} {personal} {mode:?}"
            );
            setup.adapter.stop().await.unwrap();
        }
    }
}

#[tokio::test]
async fn a_native_archive_is_refused_before_anything_is_asked_of_the_harness() {
    let mut setup = setup(working());
    setup
        .adapter
        .start(setup.start_request(), setup.sink.clone())
        .await
        .unwrap();
    let before = setup.fake.state.frames().len();
    let spec = plain_spec(
        "/workspace/web",
        new_id(),
        new_id(),
        new_id(),
        new_id(),
        SessionMode::ImportNative,
        None,
    );
    let error = setup.adapter.open_session(spec).await.unwrap_err();
    assert_eq!(error.code(), "unsupported", "{error}");
    assert_eq!(setup.fake.state.frames().len(), before);
    for kind in [ExportKind::Native, ExportKind::Neutral] {
        let error = setup
            .adapter
            .export_session(new_id(), kind, None)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "unsupported", "{kind:?}: {error}");
    }
    setup.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_carried_history_is_told_to_the_new_thread_in_pages_and_in_order() {
    use orch_adapter_codex::history::INJECT_PAGE_BYTES;
    let mut setup = setup(working());
    setup
        .adapter
        .start(setup.start_request(), setup.sink.clone())
        .await
        .unwrap();
    // Forty turns of about 20 KiB each: far more than one request carries.
    let turns = 40;
    let long = "w".repeat(20 * 1024);
    let mut lines = Vec::new();
    for turn in 1..=turns {
        for (kind, payload) in [
            (
                "user_message",
                json!({ "parts": [ { "type": "text", "text": format!("ask {turn} {long}") } ] }),
            ),
            (
                "agent_message",
                json!({ "text": format!("answer {turn}"), "text_len": 8, "phase": "final" }),
            ),
        ] {
            lines.push(
                json!({ "kind": kind, "turn_id": format!("t{turn}"), "stream": "main",
                    "status": "completed", "payload": payload })
                .to_string(),
            );
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("history.jsonl");
    let history = lines.join("\n") + "\n";
    std::fs::write(&file, &history).unwrap();
    let mut spec = plain_spec(
        "/workspace/web",
        new_id(),
        new_id(),
        new_id(),
        new_id(),
        SessionMode::ImportNeutral,
        None,
    );
    spec.history = Some(orch_agent_contract::session::LocalFile {
        path: file.to_string_lossy().into_owned(),
        size: i64::try_from(history.len()).unwrap(),
    });
    let opened = setup.adapter.open_session(spec).await.unwrap();
    let report = opened.import_report.expect("an import is reported");
    assert_eq!(report.by_kind["user_message"].kept, turns);
    assert_eq!(report.by_kind["agent_message"].kept, turns);
    let calls = setup.fake.state.calls("thread/inject_items");
    assert!(calls.len() > 1, "{} requests", calls.len());
    let mut seen = Vec::new();
    for call in &calls {
        let items = call["params"]["items"].as_array().unwrap();
        let bytes: usize = items.iter().map(|item| item.to_string().len()).sum();
        let largest = items
            .iter()
            .map(|item| item.to_string().len())
            .max()
            .unwrap();
        assert!(
            bytes < INJECT_PAGE_BYTES + largest,
            "a request carried {bytes} bytes"
        );
        seen.extend(
            items
                .iter()
                .map(|item| item["content"][0]["text"].to_string()),
        );
    }
    assert_eq!(seen.len(), usize::try_from(turns * 2).unwrap());
    assert!(seen[0].contains("ask 1 ") && seen[1].contains("answer 1"));
    assert!(seen.last().unwrap().contains("answer 40"));
    setup.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_harness_older_than_the_range_is_refused_and_a_newer_one_is_untested() {
    for version in ["0.159.0", "0.162.1"] {
        let mut old = setup(FakeLauncher::plain());
        old.fake.state.set_version(version);
        let error = old
            .adapter
            .start(old.start_request(), old.sink.clone())
            .await
            .unwrap_err();
        assert_eq!(error.code(), "harness_unavailable");
        assert!(
            error.to_string().contains(version)
                && error
                    .to_string()
                    .contains(orch_adapter_codex::descriptor::SUPPORTED_RANGE),
            "{error}"
        );
    }

    let mut new = setup(FakeLauncher::plain());
    new.fake.state.set_version("0.170.2");
    let started = new
        .adapter
        .start(new.start_request(), new.sink.clone())
        .await
        .unwrap();
    assert_eq!(started.descriptor.harness.status, HarnessSupport::Untested);
    assert_eq!(started.harness_version, "0.170.2");
    new.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_harness_that_ends_while_it_starts_says_why() {
    let mut dying = setup(FakeLauncher::plain());
    dying
        .fake
        .state
        .die_on_initialize("Error: could not open the state database");
    let error = dying
        .adapter
        .start(dying.start_request(), dying.sink.clone())
        .await
        .unwrap_err();
    assert_eq!(error.code(), "harness_unavailable");
    assert!(
        error
            .to_string()
            .contains("could not open the state database"),
        "{error}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_busy_native_initialize_can_finish_after_the_old_default() {
    let mut setup = setup(FakeLauncher::plain());
    setup.fake.state.delay_initialize(Duration::from_secs(75));
    let began = tokio::time::Instant::now();
    let started = setup
        .adapter
        .start(setup.start_request(), setup.sink.clone())
        .await
        .unwrap();
    assert_eq!(started.harness_version, "0.165.0");
    assert!(began.elapsed() >= Duration::from_secs(75));
    assert_eq!(setup.fake.state.kills.load(Ordering::SeqCst), 0);
    setup.adapter.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_configured_initialize_timeout_kills_the_process_and_keeps_the_request_failure() {
    let mut setup = setup(FakeLauncher::plain());
    setup.fake.state.delay_initialize(Duration::from_secs(3));
    let mut request = setup.start_request();
    request.provider_config = json!({ "initialize_timeout_secs": 1 });
    let error = setup
        .adapter
        .start(request, setup.sink.clone())
        .await
        .unwrap_err();
    assert_eq!(error.code(), "harness_unavailable");
    assert!(
        error
            .to_string()
            .contains("did not answer initialize within 1 seconds"),
        "{error}"
    );
    assert_eq!(setup.fake.state.kills.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn starting_again_with_the_same_request_changes_nothing_and_with_another_starts_afresh() {
    let mut setup = setup(FakeLauncher::plain());
    let start = setup.start_request();
    let first = setup
        .adapter
        .start(start.clone(), setup.sink.clone())
        .await
        .unwrap();
    let again = setup
        .adapter
        .start(start.clone(), setup.sink.clone())
        .await
        .unwrap();
    assert_eq!(first, again);
    assert_eq!(
        setup.fake.state.launches.load(Ordering::SeqCst),
        1,
        "idempotent"
    );
    let mut other = start;
    other.provider_config = json!({ "workspace_id": "ws_1" });
    setup
        .adapter
        .start(other, setup.sink.clone())
        .await
        .unwrap();
    assert_eq!(setup.fake.state.launches.load(Ordering::SeqCst), 2);
    assert!(
        std::fs::read_to_string(setup.home.path().join("config.toml"))
            .unwrap()
            .contains("ws_1"),
        "the new settings are in the home"
    );
    setup.adapter.stop().await.unwrap();
    assert!(setup.adapter.probe().await.is_err());
    setup.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn the_harness_is_started_with_the_token_in_its_environment_and_not_in_a_word_of_its_command()
{
    let mut setup = setup(FakeLauncher::plain());
    setup
        .adapter
        .start(setup.start_request(), setup.sink.clone())
        .await
        .unwrap();
    let launched = setup.fake.state.requests.lock().unwrap().clone();
    let request = &launched[0];
    assert_eq!(request.program, "codex");
    assert_eq!(
        request.args,
        ["app-server", "--listen", "stdio://"],
        "a home of Orch's own has its settings in a file"
    );
    assert_eq!(
        request.env.get("ORCH_LLM_TOKEN").map(String::as_str),
        Some("secret-token")
    );
    assert_eq!(
        request.env.get("ORCH_MCP_TOKEN").map(String::as_str),
        Some("secret-token")
    );
    assert_eq!(
        request.env.get("CODEX_HOME"),
        Some(&setup.home.path().to_string_lossy().into_owned())
    );
    assert!(request.args.iter().all(|arg| !arg.contains("secret-token")));
    setup.adapter.stop().await.unwrap();

    // On a person's machine nothing of theirs is written and the settings go as overrides.
    let mut theirs = setup_with_no_home();
    let start = theirs.start_request_no_home();
    let sink = theirs.sink.clone();
    theirs.adapter.start(start, sink).await.unwrap();
    let launched = theirs.fake.state.requests.lock().unwrap().clone();
    let args = &launched[0].args;
    assert!(
        args.windows(2)
            .any(|pair| pair == ["-c", "analytics.enabled=false"]),
        "{args:?}"
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["-c", "features.memories=false"]),
        "{args:?}"
    );
    assert!(
        !launched[0].env.contains_key("CODEX_HOME"),
        "the harness's own home"
    );
    theirs.adapter.stop().await.unwrap();
}

struct NoHome(Setup);
impl std::ops::Deref for NoHome {
    type Target = Setup;
    fn deref(&self) -> &Setup {
        &self.0
    }
}
impl std::ops::DerefMut for NoHome {
    fn deref_mut(&mut self) -> &mut Setup {
        &mut self.0
    }
}
fn setup_with_no_home() -> NoHome {
    NoHome(setup(FakeLauncher::plain()))
}
impl NoHome {
    fn start_request_no_home(&self) -> AgentStart {
        let mut start = self.0.start_request();
        start.home = None;
        start
    }
}

#[tokio::test]
async fn a_request_for_more_permissions_is_granted_for_a_turn_or_refused() {
    let mut setup = setup(working());
    let (thread, _) = running_turn(&mut setup).await;
    setup.fake.state.ask(
        7,
        "item/permissions/requestApproval",
        json!({ "threadId": "thr_1", "turnId": "turn_1", "itemId": "perm_1", "environmentId": null, "startedAtMs": 1,
                "cwd": "/workspace/web", "reason": "to fetch a package", "permissions": { "network": { "enabled": true }, "fileSystem": null } }),
    );
    let events = setup
        .until(|event| {
            matches!(
                event,
                AdapterEvent::Session {
                    event: SessionEvent::ApprovalRequested(_),
                    ..
                }
            )
        })
        .await;
    let Some(SessionEvent::ApprovalRequested(request)) = session_events(&events).pop() else {
        panic!("a request");
    };
    assert_ne!(
        request.request_ref, "7",
        "the reference is not the harness's own number, which every process starts again"
    );
    assert!(
        request.request_ref.ends_with(":7"),
        "{}",
        request.request_ref
    );
    assert_eq!(
        request.request.request_kind,
        HarnessRequestKind::Permissions
    );
    assert!(
        matches!(&request.request.detail, HarnessRequestDetail::Permissions { reason, .. } if reason.as_deref() == Some("to fetch a package"))
    );
    setup
        .adapter
        .resolve_approval(
            thread,
            &request.request_ref,
            ApprovalDecision::Option {
                option_id: "once".into(),
                note: None,
            },
        )
        .await
        .unwrap();
    wait_for_answer(&setup, 7).await;
    assert_eq!(
        setup.fake.state.answer_to(7).unwrap()["result"],
        json!({ "permissions": { "network": { "enabled": true }, "fileSystem": null }, "scope": "turn" })
    );
    // Answered twice, it is answered once.
    setup
        .adapter
        .resolve_approval(
            thread,
            &request.request_ref,
            ApprovalDecision::Option {
                option_id: "deny".into(),
                note: None,
            },
        )
        .await
        .unwrap();
    // The harness's note that it is settled is no withdrawal after an answer.
    setup.fake.state.notify(
        "serverRequest/resolved",
        json!({ "threadId": "thr_1", "requestId": 7 }),
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!kinds(&session_events(&setup.drain())).contains(&"approval.closed"));
    setup.adapter.stop().await.unwrap();
}

async fn wait_for_answer(setup: &Setup, id: i64) {
    for _ in 0..100 {
        if setup.fake.state.answer_to(id).is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("no answer to request {id}");
}

#[tokio::test]
async fn bypass_answers_residual_permissions_but_still_asks_the_person_questions_and_forms() {
    let mut setup = setup(working());
    let (thread, _) = running_turn_in_mode(&mut setup, ApprovalMode::Bypass, false).await;
    for (id, method, fields, answer) in [
        (
            91,
            "item/commandExecution/requestApproval",
            json!({"itemId":"cmd", "command":"rm -rf $danger", "cwd":"/workspace/web"}),
            json!({"decision":"accept"}),
        ),
        (
            92,
            "item/fileChange/requestApproval",
            json!({"itemId":"edit"}),
            json!({"decision":"accept"}),
        ),
        (
            93,
            "item/permissions/requestApproval",
            json!({"itemId":"permissions", "permissions":{"network":{"enabled":true}}}),
            json!({"permissions":{"network":{"enabled":true}}, "scope":"turn"}),
        ),
    ] {
        let mut params = fields;
        params["threadId"] = json!("thr_1");
        params["turnId"] = json!("turn_1");
        setup.fake.state.ask(id, method, params);
        wait_for_answer(&setup, id).await;
        assert_eq!(setup.fake.state.answer_to(id).unwrap()["result"], answer);
        assert!(
            !session_events(&setup.drain())
                .iter()
                .any(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
        );
    }

    for (id, method, fields, kind) in [
        (
            94,
            "item/tool/requestUserInput",
            json!({"itemId":"question", "questions":[{"id":"q", "header":"Choice", "question":"Which?", "options":null}]}),
            HarnessRequestKind::UserInput,
        ),
        (
            95,
            "mcpServer/elicitation/request",
            json!({"serverName":"fixture", "mode":"form", "message":"Choose", "requestedSchema":{"type":"object"}}),
            HarnessRequestKind::McpElicitation,
        ),
    ] {
        let mut params = fields;
        params["threadId"] = json!("thr_1");
        params["turnId"] = json!("turn_1");
        setup.fake.state.ask(id, method, params);
        let events = setup
            .until(|event| {
                matches!(
                    event,
                    AdapterEvent::Session {
                        event: SessionEvent::ApprovalRequested(_),
                        ..
                    }
                )
            })
            .await;
        let Some(SessionEvent::ApprovalRequested(request)) = session_events(&events).pop() else {
            panic!("a request for the person")
        };
        assert_eq!(request.request.request_kind, kind);
        assert!(setup.fake.state.answer_to(id).is_none());
        let decision = if kind == HarnessRequestKind::UserInput {
            ApprovalDecision::Answers {
                answers: std::collections::BTreeMap::from([("q".into(), vec!["First".into()])]),
                note: None,
            }
        } else {
            ApprovalDecision::Elicitation {
                action: ElicitationAction::Accept,
                content: Some(json!({})),
            }
        };
        setup
            .adapter
            .resolve_approval(thread, &request.request_ref, decision)
            .await
            .unwrap();
        wait_for_answer(&setup, id).await;
    }
    setup.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn bypass_never_grants_a_stale_request_or_an_action_in_a_read_only_plan() {
    let mut setup = setup(working());
    running_turn_in_mode(&mut setup, ApprovalMode::Bypass, true).await;
    for (id, turn) in [(96, "turn_1"), (97, "earlier-turn")] {
        setup.fake.state.ask(id, "item/commandExecution/requestApproval", json!({
            "threadId":"thr_1", "turnId":turn, "itemId":"cmd", "command":"touch forbidden", "cwd":"/workspace/web"
        }));
        wait_for_answer(&setup, id).await;
        assert_eq!(
            setup.fake.state.answer_to(id).unwrap()["result"],
            json!({"decision":"cancel"})
        );
        assert!(
            !session_events(&setup.drain())
                .iter()
                .any(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
        );
    }
    setup.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn bypass_refuses_a_permission_request_that_names_no_turn() {
    let mut setup = setup(working());
    running_turn_in_mode(&mut setup, ApprovalMode::Bypass, false).await;
    setup.fake.state.ask(
        98,
        "item/commandExecution/requestApproval",
        json!({"threadId":"thr_1", "itemId":"cmd", "command":"touch forbidden", "cwd":"/workspace/web"}),
    );
    wait_for_answer(&setup, 98).await;
    assert_eq!(
        setup.fake.state.answer_to(98).unwrap()["result"],
        json!({"decision":"cancel"})
    );
    assert!(
        !session_events(&setup.drain())
            .iter()
            .any(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
    );
    setup.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn scale_a_million_bypass_requests_keep_only_one_bounded_response_batch() {
    let mut setup = setup(working());
    running_turn_in_mode(&mut setup, ApprovalMode::Bypass, false).await;
    setup.fake.state.received.lock().unwrap().clear();
    tokio::time::timeout(Duration::from_secs(600), async {
        for batch in 0..1_000 {
            for offset in 0..1_000 {
                let id = 10_000 + batch * 1_000 + offset;
                let (method, mut params) = match offset % 3 {
                    0 => (
                        "item/commandExecution/requestApproval",
                        json!({"itemId":"cmd", "command":"rm -rf $danger", "cwd":"/workspace/web"}),
                    ),
                    1 => ("item/fileChange/requestApproval", json!({"itemId":"edit"})),
                    _ => (
                        "item/permissions/requestApproval",
                        json!({"itemId":"permissions", "permissions":{"network":{"enabled":true}}}),
                    ),
                };
                params["threadId"] = json!("thr_1");
                params["turnId"] = json!("turn_1");
                setup.fake.state.ask(id, method, params);
            }
            wait_for_answer(&setup, 10_999 + batch * 1_000).await;
            // The fixture itself retains at most one batch; it cannot turn the scale case into an
            // unbounded recorder or repeatedly clone every response since the start of the session.
            let frames = std::mem::take(&mut *setup.fake.state.received.lock().unwrap());
            assert_eq!(frames.len(), 1_000);
            for (offset, frame) in frames.iter().enumerate() {
                assert_eq!(
                    frame["id"],
                    json!(10_000 + batch * 1_000 + i64::try_from(offset).unwrap())
                );
                let expected = if offset % 3 == 2 {
                    json!({"permissions":{"network":{"enabled":true}},"scope":"turn"})
                } else {
                    json!({"decision":"accept"})
                };
                assert_eq!(frame["result"], expected);
            }
            assert!(
                !session_events(&setup.drain())
                    .iter()
                    .any(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
            );
        }
    })
    .await
    .expect("the dedicated million-request case finishes within ten minutes");
    setup.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_late_native_request_cannot_ask_on_behalf_of_a_different_turn() {
    let mut setup = setup(working());
    running_turn(&mut setup).await;
    for (id, method, fields, refusal) in [
        (
            81,
            "item/tool/requestUserInput",
            json!({"itemId":"old-question","questions":[{"id":"choice","header":"Choice","question":"Old question?","options":null}]}),
            json!({"answers":{"choice":{"answers":[]}}}),
        ),
        (
            82,
            "item/commandExecution/requestApproval",
            json!({"itemId":"old-command","command":"echo old","cwd":"/workspace"}),
            json!({"decision":"cancel"}),
        ),
        (
            83,
            "mcpServer/elicitation/request",
            json!({"serverName":"fixture","mode":"form","message":"Old form?","requestedSchema":{"type":"object"}}),
            json!({"action":"cancel","content":null,"_meta":null}),
        ),
    ] {
        let mut params = fields;
        params["threadId"] = json!("thr_1");
        params["turnId"] = json!("older-turn");
        setup.fake.state.ask(id, method, params);
        wait_for_answer(&setup, id).await;
        assert_eq!(setup.fake.state.answer_to(id).unwrap()["result"], refusal);
        assert!(
            !session_events(&setup.drain()).iter().any(|event| matches!(
                event,
                SessionEvent::ApprovalRequested(_) | SessionEvent::ItemStarted(_)
            )),
            "a stale native request cannot become current work or ask the current person"
        );
    }
    setup.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn questions_for_the_person_are_asked_and_answered_by_their_ids() {
    let mut setup = setup(working());
    let (thread, _) = running_turn(&mut setup).await;
    setup.fake.state.ask(
        3,
        "item/tool/requestUserInput",
        json!({ "threadId": "thr_1", "turnId": "turn_1", "itemId": "q_1", "isBlocking": true, "autoResolutionMs": null,
                "questions": [ { "id": "db", "header": "Database", "question": "Which database should it use?", "isOther": true,
                                 "isSecret": false, "options": [ { "label": "Postgres", "description": "The default" },
                                                                  { "label": "SQLite", "description": "A file" } ] } ] }),
    );
    let events = setup
        .until(|event| {
            matches!(
                event,
                AdapterEvent::Session {
                    event: SessionEvent::ApprovalRequested(_),
                    ..
                }
            )
        })
        .await;
    let Some(SessionEvent::ApprovalRequested(request)) = session_events(&events).pop() else {
        panic!("a request");
    };
    assert_eq!(request.title, "Which database should it use?");
    let HarnessRequestDetail::UserInput { questions, .. } = &request.request.detail else {
        panic!("questions");
    };
    assert_eq!(questions[0].options.as_ref().unwrap().len(), 2);
    assert!(request.request.options.is_empty());
    setup
        .adapter
        .resolve_approval(
            thread,
            &request.request_ref,
            ApprovalDecision::Answers {
                answers: [("db".to_owned(), vec!["Postgres".to_owned()])]
                    .into_iter()
                    .collect(),
                note: None,
            },
        )
        .await
        .unwrap();
    wait_for_answer(&setup, 3).await;
    assert_eq!(
        setup.fake.state.answer_to(3).unwrap()["result"],
        json!({ "answers": { "db": { "answers": ["Postgres"] } } })
    );
    assert!(session_events(&events).iter().any(|event| matches!(event,
        SessionEvent::ItemStarted(item) if item.native_ref == "q_1" && item.turn_id == request.turn_id)));
    assert!(
        !session_events(&setup.drain())
            .iter()
            .any(|event| matches!(event,
        SessionEvent::ItemCompleted(item) if item.native_ref == "q_1")),
        "writing an answer alone cannot complete the native source"
    );
    setup.fake.state.notify(
        "serverRequest/resolved",
        json!({"threadId":"thr_1","requestId":3}),
    );
    let completed = setup.until(|event| matches!(event,
        AdapterEvent::Session { event: SessionEvent::ItemCompleted(item), .. } if item.native_ref == "q_1")).await;
    let ended = session_events(&completed);
    let Some(SessionEvent::ItemCompleted(item)) = ended.last() else {
        panic!("native resolution")
    };
    assert_eq!(
        item.status,
        orch_agent_contract::events::ItemStatus::Completed
    );
    let call: orch_agent_contract::events::ToolCallPayload =
        serde_json::from_value(item.payload.clone()).unwrap();
    assert_eq!(
        serde_json::from_str::<Json>(&call.raw.output.unwrap().inline).unwrap(),
        setup.fake.state.answer_to(3).unwrap()["result"]
    );
    // An answer of the wrong kind is refused and does not answer the request.
    setup.fake.state.ask(
        4,
        "mcpServer/elicitation/request",
        json!({ "threadId": "thr_1", "turnId": "turn_1", "serverName": "github", "mode": "form",
                "message": "Which repository?", "requestedSchema": { "type": "object" }, "_meta": null }),
    );
    let form_events = setup
        .until(|event| {
            matches!(
                event,
                AdapterEvent::Session {
                    event: SessionEvent::ApprovalRequested(_),
                    ..
                }
            )
        })
        .await;
    let Some(SessionEvent::ApprovalRequested(form)) = session_events(&form_events).pop() else {
        panic!("a form");
    };
    let wrong = setup
        .adapter
        .resolve_approval(
            thread,
            &form.request_ref,
            ApprovalDecision::Option {
                option_id: "once".into(),
                note: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(wrong.code(), "invalid_decision");
    assert!(setup.fake.state.answer_to(4).is_none());
    setup
        .adapter
        .resolve_approval(
            thread,
            &form.request_ref,
            ApprovalDecision::Elicitation {
                action: ElicitationAction::Accept,
                content: Some(json!({ "repo": "orch" })),
            },
        )
        .await
        .unwrap();
    wait_for_answer(&setup, 4).await;
    assert_eq!(
        setup.fake.state.answer_to(4).unwrap()["result"],
        json!({ "action": "accept", "content": { "repo": "orch" }, "_meta": null })
    );
    setup.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_request_nobody_can_show_is_refused_at_once_and_one_the_adapter_does_not_know_is_an_error()
 {
    let mut setup = setup(working());
    setup
        .adapter
        .start(setup.start_request(), setup.sink.clone())
        .await
        .unwrap();
    // No session, no turn: nobody can be asked.
    setup.fake.state.ask(
        1,
        "item/commandExecution/requestApproval",
        json!({ "threadId": "not-ours", "turnId": "t", "itemId": "i", "command": "rm -rf /", "cwd": "/" }),
    );
    wait_for_answer(&setup, 1).await;
    assert_eq!(
        setup.fake.state.answer_to(1).unwrap()["result"],
        json!({ "decision": "cancel" })
    );
    setup.fake.state.ask(
        2,
        "item/tool/call",
        json!({ "threadId": "not-ours", "tool": "x", "arguments": {} }),
    );
    wait_for_answer(&setup, 2).await;
    assert_eq!(
        setup.fake.state.answer_to(2).unwrap()["error"]["code"],
        -32601
    );
    setup.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_withdrawn_request_is_reported_closed_before_the_turn_that_took_it_with_it() {
    let mut setup = setup(working());
    let (thread, turn) = running_turn(&mut setup).await;
    setup.fake.state.ask(
        9,
        "item/commandExecution/requestApproval",
        json!({ "threadId": "thr_1", "turnId": "turn_1", "itemId": "c_1", "command": "ls", "cwd": "/workspace/web",
                "availableDecisions": ["accept", "cancel"] }),
    );
    let asked = setup
        .until(|event| {
            matches!(
                event,
                AdapterEvent::Session {
                    event: SessionEvent::ApprovalRequested(_),
                    ..
                }
            )
        })
        .await;
    let Some(SessionEvent::ApprovalRequested(withdrawn)) = session_events(&asked).pop() else {
        panic!("a request");
    };
    setup.fake.state.notify(
        "turn/completed",
        json!({ "threadId": "thr_1", "turn": { "id": "turn_1", "status": "interrupted", "error": null } }),
    );
    let events = setup
        .until(|event| {
            matches!(
                event,
                AdapterEvent::Session {
                    event: SessionEvent::TurnCompleted(_),
                    ..
                }
            )
        })
        .await;
    assert_eq!(
        kinds(&session_events(&events)),
        ["approval.closed", "turn.completed"]
    );
    setup.fake.state.notify(
        "serverRequest/resolved",
        json!({ "threadId": "thr_1", "requestId": 9 }),
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !session_events(&setup.drain())
            .iter()
            .any(|event| matches!(event, SessionEvent::ApprovalClosed(_))),
        "the withdrawn request is not reported twice; separate cleanup observations may follow"
    );
    let late = setup
        .adapter
        .resolve_approval(
            thread,
            &withdrawn.request_ref,
            ApprovalDecision::Option {
                option_id: "once".into(),
                note: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(late.code(), "not_found");
    let _ = turn;
    setup.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_turn_states_the_model_the_effort_the_confinement_and_the_environment_every_time() {
    let mut setup = setup(working());
    running_turn(&mut setup).await;
    let started = setup.fake.state.calls("turn/start");
    let params = &started[0]["params"];
    assert_eq!(params["threadId"], "thr_1");
    assert_eq!(params["model"], "mock-large");
    assert_eq!(params["effort"], "high");
    assert_eq!(params["approvalPolicy"], "untrusted");
    assert_eq!(params["sandboxPolicy"]["type"], "workspaceWrite");
    assert_eq!(
        params["sandboxPolicy"]["writableRoots"],
        json!(["/workspace/web"])
    );
    assert_eq!(
        params["serviceTierForTurn"], "default",
        "no other speed was chosen, and none is left from a turn before"
    );
    assert_eq!(params["cwd"], "/workspace/web");
    assert_eq!(
        params["input"][0],
        json!({ "type": "text", "text": "go", "text_elements": [] })
    );
    assert!(params["clientUserMessageId"].is_string());
    let opened = setup.fake.state.calls("thread/start");
    assert_eq!(opened[0]["params"]["approvalPolicy"], "never");
    assert_eq!(opened[0]["params"]["sandbox"], "danger-full-access");
    assert_eq!(
        opened[0]["params"]["developerInstructions"],
        "You are working inside Orch."
    );
    setup.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn the_frames_of_a_session_are_kept_as_raw_frames_when_it_asks_for_them() {
    let mut setup = setup(working());
    setup
        .adapter
        .start(setup.start_request(), setup.sink.clone())
        .await
        .unwrap();
    let thread = new_id();
    let mut spec = plain_spec(
        "/workspace/web",
        new_id(),
        new_id(),
        new_id(),
        thread,
        SessionMode::New,
        None,
    );
    spec.settings.raw_events = true;
    setup.adapter.open_session(spec).await.unwrap();
    setup.fake.state.notify(
        "some/unknown/notification",
        json!({ "threadId": "thr_1", "anything": [1, 2, 3] }),
    );
    let turn = TurnStart {
        plan: false,
        thread_id: thread,
        turn_id: new_id(),
        text: "go".into(),
        images: Vec::new(),
        model: "mock-large".into(),
        effort: None,
        params: json!({}),
        approval_mode: orch_agent_contract::descriptor::ApprovalMode::Bypass,
        sandbox_mode: orch_agent_contract::descriptor::SandboxMode::Open,
    };
    setup.adapter.start_turn(turn).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let events = session_events(&setup.drain());
    let raw: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::RawFrame(frame) => Some(frame),
            _ => None,
        })
        .collect();
    assert!(
        raw.iter()
            .any(|frame| frame.direction == "out" && frame.payload["method"] == "turn/start")
    );
    assert!(
        raw.iter().any(|frame| frame.direction == "in"
            && frame.payload["method"] == "some/unknown/notification"),
        "what the adapter does not know is kept, and is no error"
    );
    setup.adapter.stop().await.unwrap();

    // A session that does not ask has none.
    let mut quiet = setup_quiet();
    quiet
        .adapter
        .start(quiet.start_request(), quiet.sink.clone())
        .await
        .unwrap();
    let spec = plain_spec(
        "/workspace/web",
        new_id(),
        new_id(),
        new_id(),
        new_id(),
        SessionMode::New,
        None,
    );
    quiet.adapter.open_session(spec).await.unwrap();
    quiet
        .fake
        .state
        .notify("some/unknown/notification", json!({ "threadId": "thr_1" }));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!kinds(&session_events(&quiet.drain())).contains(&"raw.frame"));
    quiet.adapter.stop().await.unwrap();
}

fn setup_quiet() -> Setup {
    setup(working())
}

#[tokio::test]
async fn a_sign_in_by_a_code_shows_an_address_and_a_code_and_ends_as_the_harness_says() {
    let launcher = FakeLauncher::new(|method, _| match method {
        "account/login/start" => Ok(json!({ "type": "chatgptDeviceCode", "loginId": "login_1",
                                            "verificationUrl": "https://sign-in.example.test/device", "userCode": "ABCD-1234" })),
        _ => Ok(json!({})),
    });
    let mut setup = setup(launcher);
    setup
        .adapter
        .start(setup.start_request(), setup.sink.clone())
        .await
        .unwrap();
    setup.drain();
    setup
        .adapter
        .login(LoginRequest {
            method: "device_code".into(),
            values: Default::default(),
            config: json!({}),
        })
        .await
        .unwrap();
    let AdapterEvent::Login(shown) = setup.next().await else {
        panic!("what to show");
    };
    assert_eq!(shown.state, LoginStateKind::AwaitingUser);
    assert_eq!(shown.method.as_deref(), Some("device_code"));
    assert_eq!(
        (shown.shows[0].kind, shown.shows[0].value.as_str()),
        (ShowKind::Uri, "https://sign-in.example.test/device")
    );
    assert_eq!(
        (shown.shows[1].kind, shown.shows[1].value.as_str()),
        (ShowKind::Code, "ABCD-1234")
    );
    assert!(shown.expires_at.is_some());
    assert_eq!(
        setup.fake.state.calls("account/login/start")[0]["params"],
        json!({ "type": "chatgptDeviceCode" })
    );

    // Entered on the vendor's page: the harness says so, and the sign-in is done.
    setup.fake.state.notify(
        "account/login/completed",
        json!({ "loginId": "login_1", "success": true, "error": null }),
    );
    let AdapterEvent::Login(done) = setup.next().await else {
        panic!("how it ended");
    };
    assert_eq!(done.state, LoginStateKind::Completed);

    // A sign-in that fails says why; one that is cancelled leaves none running.
    setup
        .adapter
        .login(LoginRequest {
            method: "device_code".into(),
            values: Default::default(),
            config: json!({}),
        })
        .await
        .unwrap();
    setup.next().await;
    setup.fake.state.notify(
        "account/login/completed",
        json!({ "loginId": "login_1", "success": false, "error": "the code expired" }),
    );
    let AdapterEvent::Login(failed) = setup.next().await else {
        panic!("how it ended");
    };
    assert_eq!(
        (failed.state, failed.error.as_deref()),
        (LoginStateKind::Failed, Some("the code expired"))
    );
    setup
        .adapter
        .login(LoginRequest {
            method: "device_code".into(),
            values: Default::default(),
            config: json!({}),
        })
        .await
        .unwrap();
    setup.next().await;
    setup.adapter.cancel_login().await.unwrap();
    assert_eq!(setup.fake.state.calls("account/login/cancel").len(), 1);
    let AdapterEvent::Login(idle) = setup.next().await else {
        panic!("none runs");
    };
    assert_eq!(idle.state, LoginStateKind::Idle);
    assert_eq!(
        setup
            .adapter
            .login_input(Default::default())
            .await
            .unwrap_err()
            .code(),
        "not_found"
    );
    setup.adapter.stop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_code_that_is_not_entered_in_time_expires() {
    let launcher = FakeLauncher::new(|method, _| match method {
        "account/login/start" => Ok(
            json!({ "loginId": "login_9", "verificationUrl": "https://x.example.test", "userCode": "WXYZ" }),
        ),
        _ => Ok(json!({})),
    });
    let mut setup = setup(launcher);
    setup
        .adapter
        .start(setup.start_request(), setup.sink.clone())
        .await
        .unwrap();
    setup.drain();
    setup
        .adapter
        .login(LoginRequest {
            method: "device_code".into(),
            values: Default::default(),
            config: json!({}),
        })
        .await
        .unwrap();
    let shown = setup.next().await;
    assert!(
        matches!(shown, AdapterEvent::Login(state) if state.state == LoginStateKind::AwaitingUser)
    );
    tokio::task::yield_now().await; // the task that waits for the end of the code starts to wait
    tokio::time::advance(Duration::from_secs(16 * 60)).await;
    let AdapterEvent::Login(expired) = setup.next().await else {
        panic!("how it ended");
    };
    assert_eq!(expired.state, LoginStateKind::Expired);
    setup.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn the_sign_in_of_a_machine_is_not_given_from_orch() {
    let mut setup = setup(FakeLauncher::plain());
    let mut start = setup.start_request();
    start.home = None;
    setup
        .adapter
        .start(start, setup.sink.clone())
        .await
        .unwrap();
    let error = setup
        .adapter
        .login(LoginRequest {
            method: "device_code".into(),
            values: Default::default(),
            config: json!({}),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code(), "login_on_the_machine");
    assert_eq!(
        setup.adapter.logout().await.unwrap_err().code(),
        "login_on_the_machine"
    );
    setup.adapter.stop().await.unwrap();
    let _ = HarnessState::Online;
}

/// A fake whose `config/read` says it would load two servers of its own.
fn loading_two_servers() -> FakeLauncher {
    FakeLauncher::new(|method, _| match method {
        "environment/info" => Ok(executor_info()),
        "config/read" => Ok(json!({ "config": { "model_provider": null, "mcp_servers": {
            "github": { "command": "npx", "args": ["-y", "x"], "enabled": true },
            "docs": { "url": "https://docs.example.test/mcp", "enabled": true }
        } } })),
        "thread/start" => Ok(json!({ "thread": { "id": "thr_1", "path": null } })),
        _ => Ok(json!({})),
    })
}

/// A workspace whose `.codex` holds a server of its own.
fn configured_workspace() -> tempfile::TempDir {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join(".codex")).unwrap();
    std::fs::write(
        workspace.path().join(".codex").join("config.toml"),
        "[mcp_servers.project]\ncommand = \"./serve\"\n",
    )
    .unwrap();
    workspace
}

#[tokio::test]
async fn the_servers_the_harness_says_it_loads_and_those_of_a_kept_out_projects_files_are_switched_off_for_a_session()
 {
    let workspace = configured_workspace();
    let directory = workspace.path().to_string_lossy().into_owned();
    let mut setup = setup(loading_two_servers());
    setup
        .adapter
        .start(setup.start_request(), setup.sink.clone())
        .await
        .unwrap();
    let mut spec = plain_spec(
        &directory,
        new_id(),
        new_id(),
        new_id(),
        new_id(),
        SessionMode::New,
        None,
    );
    spec.mcp = vec!["notes".to_owned()];
    spec.settings.workspace_configuration = Some(WorkspaceConfiguration {
        answer: ConfigTrust::KeepOut,
        files: orch_agent_contract::session::configuration_files(
            workspace.path(),
            &[".codex".to_owned()],
        ),
    });
    setup.adapter.open_session(spec).await.unwrap();

    let opened = setup.fake.state.calls("thread/start");
    let config = &opened[0]["params"]["config"];
    let servers = &config["mcp_servers"];
    assert_eq!(
        servers["github"],
        json!({ "command": "npx", "enabled": false }),
        "a server of the home, switched off, with the program repeated so that the entry stands alone"
    );
    assert_eq!(
        servers["docs"],
        json!({ "url": "https://docs.example.test/mcp", "enabled": false })
    );
    assert_eq!(
        servers["project"],
        json!({ "command": "./serve", "enabled": false }),
        "a server of the project's own file, kept out whether or not the harness reads that file"
    );
    assert!(servers["notes"]["url"].as_str().unwrap().contains("/mcp/"));
    assert_eq!(servers["notes"].get("enabled"), None);
    assert_eq!(
        config["projects"],
        json!({ directory.clone(): { "trust_level": "untrusted" } }),
        "the workspace is a folder the harness does not trust"
    );
    let looks: Vec<_> = setup
        .fake
        .state
        .calls("config/read")
        .into_iter()
        .map(|call| call["params"]["cwd"].clone())
        .collect();
    assert!(
        looks.contains(&json!(directory)),
        "the harness is asked what it loads where the session works: {looks:?}"
    );
    setup.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_workspace_configuration_nobody_answered_for_is_refused_one_the_harness_made_empty_is_not_and_a_trusted_one_keeps_its_servers()
 {
    let mut setup = setup(loading_two_servers());
    setup
        .adapter
        .start(setup.start_request(), setup.sink.clone())
        .await
        .unwrap();
    let open = |directory: &str| {
        plain_spec(
            directory,
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            SessionMode::New,
            None,
        )
    };

    // A `.codex` the harness made for itself and left empty is no configuration: nobody is asked about it.
    let made = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(made.path().join(".codex").join("cache")).unwrap();
    let made_directory = made.path().to_string_lossy().into_owned();
    setup
        .adapter
        .open_session(open(&made_directory))
        .await
        .unwrap();
    let started = setup.fake.state.calls("thread/start");
    assert_eq!(started.len(), 1);
    assert_eq!(
        started[0]["params"]["config"].get("projects"),
        None,
        "nothing is said of the trust of a workspace that holds no configuration"
    );

    // One that holds something, with no answer: refused before the harness is asked anything of it, with the code
    // that has the system ask the people who manage the environment, and the place found.
    let configured = configured_workspace();
    let configured_directory = configured.path().to_string_lossy().into_owned();
    let error = setup
        .adapter
        .open_session(open(&configured_directory))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "workspace_configuration", "{error}");
    assert_eq!(
        AdapterError::configuration_found(&error.detail()),
        vec![".codex".to_owned()]
    );
    let files = AdapterError::configuration_files(&error.detail());
    assert_eq!(
        files,
        orch_agent_contract::session::configuration_files(
            configured.path(),
            &[".codex".to_owned()]
        ),
        "the refusal names the files by the hash every adapter makes of them"
    );
    assert_eq!(
        setup.fake.state.calls("thread/start").len(),
        1,
        "no thread was started"
    );

    // Trusted: the harness reads it as in a folder it trusts, and its servers are its own to keep.
    let mut trusted = open(&configured_directory);
    trusted.settings.workspace_configuration = Some(WorkspaceConfiguration {
        answer: ConfigTrust::Trust,
        files: files.clone(),
    });
    setup.adapter.open_session(trusted.clone()).await.unwrap();
    let started = setup.fake.state.calls("thread/start");
    let config = &started[1]["params"]["config"];
    assert_eq!(
        config["projects"],
        json!({ configured_directory.clone(): { "trust_level": "trusted" } })
    );
    assert_eq!(config["mcp_servers"].get("project"), None);
    assert_eq!(
        config["mcp_servers"]["github"]["enabled"], false,
        "the home's servers are switched off all the same"
    );

    // The files changed since the answer: it no longer holds, and the question is asked again.
    std::fs::write(
        configured.path().join(".codex").join("config.toml"),
        "[mcp_servers.other]\ncommand = \"./other\"\n",
    )
    .unwrap();
    let mut again = open(&configured_directory);
    again.settings.workspace_configuration = trusted.settings.workspace_configuration.clone();
    let error = setup.adapter.open_session(again).await.unwrap_err();
    assert_eq!(error.code(), "workspace_configuration", "{error}");
    assert_ne!(AdapterError::configuration_files(&error.detail()), files);
    setup.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_session_whose_tools_run_elsewhere_switches_off_only_what_the_homes_configuration_loads()
{
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join(".codex")).unwrap();
    std::fs::write(
        workspace.path().join(".codex").join("config.toml"),
        "[mcp_servers.project]\ncommand = \"./serve\"\n",
    )
    .unwrap();
    let directory = workspace.path().to_string_lossy().into_owned();
    let mut setup = setup(loading_two_servers());
    setup
        .adapter
        .start(setup.start_request(), setup.sink.clone())
        .await
        .unwrap();
    let mut spec = plain_spec(
        &directory,
        new_id(),
        new_id(),
        new_id(),
        new_id(),
        SessionMode::New,
        None,
    );
    spec.exec.executor = Some("codex-exec-server".to_owned());
    setup.adapter.open_session(spec).await.unwrap();

    let opened = setup.fake.state.calls("thread/start");
    let servers = &opened[0]["params"]["config"]["mcp_servers"];
    assert_eq!(servers["github"]["enabled"], false);
    assert_eq!(
        servers.get("project"),
        None,
        "the project is on another machine: its files here are not its files"
    );
    assert!(
        setup
            .fake
            .state
            .calls("config/read")
            .iter()
            .all(|call| call["params"].get("cwd").is_none()),
        "no directory of this machine is asked about"
    );
    setup.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_remote_session_resolves_its_executor_before_opening_and_before_sending_input() {
    let mut setup = setup(working());
    setup
        .adapter
        .start(setup.start_request(), setup.sink.clone())
        .await
        .unwrap();
    let thread = new_id();
    let mut spec = plain_spec(
        "/workspace/web",
        new_id(),
        new_id(),
        new_id(),
        thread,
        SessionMode::New,
        None,
    );
    spec.exec.executor = Some("codex-exec-server".to_owned());
    setup.adapter.open_session(spec).await.unwrap();
    assert!(
        setup.fake.state.calls("turn/start").is_empty(),
        "opening sends no hidden input"
    );
    setup
        .adapter
        .start_turn(TurnStart {
            plan: false,
            thread_id: thread,
            turn_id: new_id(),
            text: "read the handoff".into(),
            images: Vec::new(),
            model: "mock-large".into(),
            effort: None,
            params: json!({}),
            approval_mode: orch_agent_contract::descriptor::ApprovalMode::Bypass,
            sandbox_mode: orch_agent_contract::descriptor::SandboxMode::Open,
        })
        .await
        .unwrap();

    let frames = setup.fake.state.frames();
    let methods: Vec<_> = frames
        .iter()
        .filter_map(|frame| {
            frame["method"].as_str().filter(|method| {
                matches!(
                    *method,
                    "environment/add" | "environment/info" | "thread/start" | "turn/start"
                )
            })
        })
        .collect();
    assert_eq!(
        methods,
        vec![
            "environment/add",
            "environment/info",
            "thread/start",
            "environment/info",
            "turn/start"
        ]
    );
    let registered =
        setup.fake.state.calls("environment/add")[0]["params"]["environmentId"].clone();
    let opened = setup.fake.state.calls("thread/start");
    let turns = setup.fake.state.calls("turn/start");
    let selection = json!([{
        "environmentId": registered,
        "cwd": "/workspace/web",
        "runtimeWorkspaceRoots": ["/workspace/web"]
    }]);
    assert_eq!(opened[0]["params"]["environments"], selection);
    assert_eq!(
        turns[0]["params"]["environments"], selection,
        "the executor's own default cwd does not replace the thread's workspace"
    );
    assert_eq!(turns[0]["params"]["input"][0]["text"], "read the handoff");
    assert!(
        setup
            .fake
            .state
            .calls("environment/info")
            .iter()
            .all(|call| call["params"]["environmentId"] == registered)
    );
    setup.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn an_executor_that_has_not_reported_a_shell_refuses_opening_before_any_input_can_run() {
    let failures: [common::fake::Answer; 3] = [
        Err((-32603, "exec-server connection attempt failed".to_owned())),
        Ok(json!({})),
        Ok(json!({ "shell": { "name": "bash", "path": "" } })),
    ];
    for failure in failures {
        let mut setup = setup(FakeLauncher::new(move |method, _| match method {
            "environment/info" => failure.clone(),
            "thread/start" => Ok(json!({ "thread": { "id": "thr_1", "path": null } })),
            _ => Ok(json!({})),
        }));
        setup
            .adapter
            .start(setup.start_request(), setup.sink.clone())
            .await
            .unwrap();
        let mut spec = plain_spec(
            "/workspace/web",
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            SessionMode::New,
            None,
        );
        spec.exec.executor = Some("codex-exec-server".to_owned());
        let error = setup.adapter.open_session(spec).await.unwrap_err();
        assert_eq!(error.code(), "environment_unreachable", "{error}");
        assert!(matches!(
            error,
            AdapterError::Other {
                retryable: true,
                ..
            }
        ));
        assert_eq!(setup.fake.state.calls("environment/add").len(), 1);
        assert!(setup.fake.state.calls("thread/start").is_empty());
        assert!(setup.fake.state.calls("turn/start").is_empty());
        setup.adapter.stop().await.unwrap();
    }
}

#[tokio::test]
async fn a_lost_executor_keeps_input_unsent_and_its_session_can_retry_after_reconnection() {
    let ready = Arc::new(AtomicBool::new(true));
    let observed = Arc::clone(&ready);
    let mut setup = setup(FakeLauncher::new(move |method, _| match method {
        "environment/info" if observed.load(Ordering::SeqCst) => Ok(executor_info()),
        "environment/info" => Err((-32603, "exec-server connection attempt failed".to_owned())),
        "thread/start" => Ok(json!({ "thread": { "id": "thr_1", "path": null } })),
        "turn/start" => Ok(json!({ "turn": { "id": "turn_1", "status": "inProgress" } })),
        _ => Ok(json!({})),
    }));
    setup
        .adapter
        .start(setup.start_request(), setup.sink.clone())
        .await
        .unwrap();
    let thread = new_id();
    let mut spec = plain_spec(
        "/workspace/web",
        new_id(),
        new_id(),
        new_id(),
        thread,
        SessionMode::New,
        None,
    );
    spec.exec.executor = Some("codex-exec-server".to_owned());
    setup.adapter.open_session(spec).await.unwrap();
    let turn = TurnStart {
        plan: false,
        thread_id: thread,
        turn_id: new_id(),
        text: "read the handoff".into(),
        images: Vec::new(),
        model: "mock-large".into(),
        effort: None,
        params: json!({}),
        approval_mode: orch_agent_contract::descriptor::ApprovalMode::Bypass,
        sandbox_mode: orch_agent_contract::descriptor::SandboxMode::Open,
    };
    ready.store(false, Ordering::SeqCst);
    let error = setup.adapter.start_turn(turn.clone()).await.unwrap_err();
    assert_eq!(error.code(), "environment_unreachable");
    assert!(
        setup.fake.state.calls("turn/start").is_empty(),
        "input is not consumed without its executor"
    );
    ready.store(true, Ordering::SeqCst);
    setup.adapter.start_turn(turn).await.unwrap();
    let sent = setup.fake.state.calls("turn/start");
    assert_eq!(
        sent.len(),
        1,
        "the failed attempt leaves no pending-turn conflict or duplicate input"
    );
    assert_eq!(sent[0]["params"]["input"][0]["text"], "read the handoff");
    assert_eq!(setup.fake.state.calls("environment/info").len(), 3);
    setup.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_profile_that_asks_to_attach_to_a_daemon_is_refused_and_nothing_is_started() {
    let mut setup = setup(working());
    let mut start = setup.start_request();
    start.profile.attach = Some(orch_agent_contract::facts::Attach {
        mode: orch_agent_contract::facts::AttachMode::Daemon,
        binary: None,
        home: None,
    });
    let error = setup
        .adapter
        .start(start, setup.sink.clone())
        .await
        .unwrap_err();
    assert_eq!(error.code(), "unsupported", "{error}");
    assert_eq!(setup.fake.state.launches.load(Ordering::SeqCst), 0);
}

//! The selected page route controls the actual native launch independently of the search route.
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
    session::{NativeModelContext, SessionMode},
};
use orch_plugin_contract::Secret;
use orch_provider_contract::capabilities::{
    Availability, BackendRef, Declarations, OfferingRef, Registry, ThreadContext, catalogue,
};
use orch_testkit::harness_corpus::{Bindings, Frame, collect_through};
use orch_types::{Json, new_id};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};

fn resolved(mode: Option<&str>) -> NativeModelContext {
    let descriptor = orch_adapter_claude_code::descriptor::descriptor("2.1.322", false);
    let provider = new_id();
    let mut registry = Registry::default();
    registry.register("common", catalogue().unwrap()).unwrap();
    let mut declarations: Declarations = serde_json::from_value(json!({
        "interfaces":{"fixture-pages":{"kind":"fetch","implementation":"fixture-pages"}},
        "providers":{(provider.to_string()):{
            "credential_roles":["model"],
            "endpoints":{
                "model":{"interface":"anthropic-messages","address":"https://model.example.invalid","credential_role":"model"},
                "pages":{"interface":"fixture-pages","address":"https://pages.example.invalid","credential_role":"model"}
            },
            "offerings":{
                "mock-large":{"endpoint":"model","kind":"model","enabled":true},
                "pages":{"endpoint":"pages","kind":"service","enabled":true,"capabilities":{"page_fetch":{"enabled":true}}}
            }
        }}
    })).unwrap();
    declarations
        .programs
        .insert(descriptor.kind.clone(), descriptor.capability_program());
    registry.register("fixture", declarations).unwrap();
    let mut context = ThreadContext {
        program: descriptor.kind,
        access: "forwarded".into(),
        current: Some(OfferingRef {
            provider,
            offering: "mock-large".into(),
        }),
        interfaces: vec!["anthropic-messages".into(), "fixture-pages".into()],
        executors: vec!["api-backed".into()],
        ..ThreadContext::default()
    };
    for offering in ["mock-large", "pages"] {
        context.availability.entry(provider).or_default().insert(
            offering.into(),
            Availability {
                allowed: true,
                ..Availability::default()
            },
        );
    }
    context
        .credential_roles
        .insert(provider, vec!["model".into()]);
    context.backends.insert(
        "page_fetch".into(),
        BackendRef {
            route: "page_fetch.service-api".into(),
            offering: Some(OfferingRef {
                provider,
                offering: "pages".into(),
            }),
            ..BackendRef::default()
        },
    );
    if let Some(mode) = mode {
        context
            .choices
            .insert("page_fetch".into(), Some(mode.into()));
    }
    let resolutions = registry.resolve(&context);
    let page = resolutions
        .iter()
        .find(|row| row.capability == "page_fetch")
        .unwrap();
    match mode {
        Some("service") => {
            let route = page
                .route
                .as_ref()
                .expect("the explicit fixture service is usable");
            assert_eq!(route.orch_tool.as_deref(), Some("web_fetch"));
            assert!(route.disable_native);
            assert_eq!(page.native_config.get("page_fetch"), Some(&json!(false)));
        }
        Some("off") => {
            assert!(page.route.is_none());
            assert_eq!(page.native_config.get("page_fetch"), Some(&json!(false)));
        }
        Some("on") | None => {
            assert_eq!(page.route.as_ref().unwrap().id, "page_fetch.program");
            assert!(page.route.as_ref().unwrap().orch_tool.is_none());
            assert_eq!(
                page.native_config.get("page_fetch").cloned(),
                mode.map(|_| Json::Bool(true))
            );
        }
        _ => panic!("the fixture names only actual page modes"),
    }
    NativeModelContext::captured("mock-large".into(), Default::default(), &resolutions).unwrap()
}

fn version() -> Vec<Frame> {
    vec![
        Frame::Raw {
            line: "2.1.322 (Claude Code)".into(),
            newline: true,
        },
        Frame::Exit { code: Some(0) },
    ]
}
fn inspect() -> Vec<Frame> {
    let mut frames = handshake()[..2].to_vec();
    frames.push(to_native(json!({"type":"control_request","request_id":{"$replay_capture":"settings"},"request":{"subtype":"get_settings"}})));
    frames.push(from_native(json!({"type":"control_response","response":{"subtype":"success","request_id":{"$replay_ref":"settings"},"response":{"effective":{}}}})));
    frames
}
fn offered(
    launch: &orch_adapter_claude_code::process::LaunchSpec,
    separated: bool,
    tool: &str,
) -> bool {
    if separated {
        let list = launch
            .args
            .windows(2)
            .find(|pair| pair[0] == "--tools")
            .unwrap();
        list[1].split(',').any(|name| name == tool)
    } else {
        let at = launch
            .args
            .iter()
            .position(|arg| arg == "--disallowed-tools")
            .unwrap();
        !launch.args[at + 1..].iter().any(|name| name == tool)
    }
}
async fn start(
    scripts: Arc<Scripts>,
    home: &tempfile::TempDir,
) -> (ClaudeCodeAdapter, orch_agent_contract::adapter::EventStream) {
    let (sink, events) = EventSink::channel();
    let mut adapter = ClaudeCodeAdapter::with_launcher(Arc::new(scripts.clock.clone()), scripts);
    adapter
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
                    env: BTreeMap::new(),
                },
                home: Some(home.path().to_str().unwrap().into()),
                provider_config: json!({}),
                loopback: Loopback::new(1, Secret::new("offline-page-route")),
                binary: Some("page-route-candidate".into()),
            },
            sink,
        )
        .await
        .unwrap();
    (adapter, events)
}

#[tokio::test]
async fn native_and_service_page_modes_control_actual_launches_without_coupling_search() {
    for separated in [false, true] {
        for page_mode in [None, Some("on"), Some("service"), Some("off")] {
            for search_mode in ["service", "live"] {
                let mut frames = inspect();
                frames.extend(barrier());
                let scripts = Scripts::new(
                    vec![version(), frames],
                    orch_testkit::clock::manual(),
                    Bindings::new(),
                );
                let home = tempfile::tempdir().unwrap();
                let work = tempfile::tempdir().unwrap();
                let (mut adapter, mut events) = start(scripts.clone(), &home).await;
                let thread = new_id();
                let mut spec = common::fixture_spec(
                    thread,
                    new_id(),
                    work.path().to_str().unwrap(),
                    "offline-page-route",
                    SessionMode::New,
                    None,
                );
                spec.settings.native_model = Some(Box::new(resolved(page_mode)));
                // A public unknown parameter is not the private resolved native switch.
                spec.settings.params = json!({"web_search":search_mode,"page_fetch":false});
                if separated {
                    spec.exec.executor = Some("orch-exec".into());
                    spec.exec.tool_set = Some("claude-code".into());
                }
                adapter.open_session(spec).await.unwrap();
                adapter.warm_session(thread).await.unwrap();
                let _ = collect_through(&mut events, "item.completed", WAIT)
                    .await
                    .unwrap();
                scripts.completed().await;
                adapter.stop().await.unwrap();
                scripts.finish().await;
                let launches = scripts.launched.lock().unwrap();
                let launch = launches
                    .iter()
                    .find(|launch| launch.label.starts_with("session-"))
                    .unwrap();
                assert_eq!(
                    offered(launch, separated, "WebFetch"),
                    !matches!(page_mode, Some("service" | "off")),
                    "{separated}/{page_mode:?}/{search_mode}"
                );
                assert_eq!(
                    offered(launch, separated, "WebSearch"),
                    search_mode == "live"
                );
                for tool in ["Agent", "AskUserQuestion", "ExitPlanMode", "TaskList"] {
                    assert!(offered(launch, separated, tool));
                }
                assert!(!offered(launch, separated, "ScheduleWakeup"));
                if separated {
                    assert!(!offered(launch, true, "Bash"));
                }
            }
        }
    }
}

fn next_turn(
    thread: orch_types::Id,
    native: NativeModelContext,
) -> orch_agent_contract::session::TurnStart {
    orch_agent_contract::session::TurnStart {
        native_model: Some(Box::new(native)),
        thread_id: thread,
        turn_id: new_id(),
        text: "Selected page reader".into(),
        images: vec![],
        native_wake: None,
        plan: false,
        model: "mock-large".into(),
        effort: None,
        params: json!({"web_search":"service"}),
        approval_mode: orch_agent_contract::descriptor::ApprovalMode::Ask,
        sandbox_mode: orch_agent_contract::descriptor::SandboxMode::Open,
    }
}

#[tokio::test]
async fn a_reused_session_restarts_with_only_the_selected_reader_before_taking_input() {
    for separated in [false, true] {
        let mut initial = inspect();
        initial.extend(barrier());
        let turn = next_turn(new_id(), resolved(Some("service")));
        let thread = turn.thread_id;
        let mut changed = inspect();
        changed.push(to_native(json!({"type":"user","uuid":turn.turn_id,"message":{"role":"user","content":[{"type":"text","text":turn.text}]}})));
        changed.extend(barrier());
        let scripts = Scripts::new(
            vec![version(), initial, changed],
            orch_testkit::clock::manual(),
            Bindings::new(),
        );
        let home = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let (mut adapter, mut events) = start(scripts.clone(), &home).await;
        let mut spec = common::fixture_spec(
            thread,
            new_id(),
            work.path().to_str().unwrap(),
            "offline-page-switch",
            SessionMode::New,
            None,
        );
        spec.settings.native_model = Some(Box::new(resolved(Some("on"))));
        if separated {
            spec.exec.executor = Some("orch-exec".into());
            spec.exec.tool_set = Some("claude-code".into());
        }
        adapter.open_session(spec).await.unwrap();
        adapter.warm_session(thread).await.unwrap();
        let _ = collect_through(&mut events, "item.completed", WAIT)
            .await
            .unwrap();
        scripts.completed().await;
        adapter.start_turn(turn).await.unwrap();
        scripts.completed().await;
        adapter.stop().await.unwrap();
        scripts.finish().await;
        let launches = scripts.launched.lock().unwrap();
        let native: Vec<_> = launches
            .iter()
            .filter(|launch| launch.label.starts_with("session-"))
            .collect();
        assert_eq!(
            native.len(),
            2,
            "the warmed process cannot retain its previous page reader"
        );
        assert!(offered(native[0], separated, "WebFetch"));
        assert!(!offered(native[1], separated, "WebFetch"));
        let traces = scripts.transcripts();
        assert!(!traces[1].frames.iter().any(
            |frame| matches!(frame, Frame::ToHarness { frame, .. } if frame["type"] == "user")
        ));
        assert_eq!(traces[2].frames.iter().filter(|frame| matches!(frame, Frame::ToHarness { frame, .. } if frame["type"] == "user")).count(), 1);
    }
}

#[tokio::test]
async fn changing_reader_with_owned_background_work_retries_without_a_second_input_or_killing_it() {
    use common::corpus::{Actor, SESSION, input};
    use orch_adapter_claude_code::{
        conversation::{self, Command},
        sessions::Origin,
    };
    let mut frames = handshake();
    frames.push(input());
    frames.push(from_native(json!({"type":"assistant","message":{"id":"message","role":"assistant","content":[{
        "type":"tool_use","id":"tool","name":"Bash","input":{"command":"fixture background","run_in_background":true}}]}})));
    frames.push(from_native(json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool","content":"Launched"}]},"tool_use_result":{"backgroundTaskId":"task"}})));
    frames.push(from_native(json!({"type":"system","subtype":"task_started","session_id":SESSION,
        "task_id":"task","run_id":"run-1","tool_use_id":"tool","task_type":"local_bash","is_backgrounded":true,"description":"Fixture task"})));
    frames.push(from_native(
        json!({"type":"result","subtype":"success","is_error":false,"num_turns":1,"result":""}),
    ));
    frames.push(to_native(json!({"type":"control_request","request_id":{"$replay_capture":"context"},"request":{"subtype":"get_context_usage","detail":"summary"}})));
    frames.push(from_native(json!({"type":"control_response","response":{"subtype":"error","request_id":{"$replay_ref":"context"},"error":"No breakdown in this fixture"}})));
    frames.extend(barrier());
    let after_turn = frames.len();
    frames.push(from_native(json!({"type":"system","subtype":"task_notification","session_id":SESSION,
        "task_id":"task","run_id":"run-1","tool_use_id":"tool","status":"completed","summary":"Fixture terminal","exit_code":0})));
    frames.extend(barrier());
    let (scripts, gate) = Scripts::paused(frames, after_turn, orch_testkit::clock::manual());
    let mut actor = Actor::start_with_scripts(scripts.clone(), Origin::New, false, false).await;
    actor.opened.as_ref().unwrap();
    let _ = collect_through(&mut actor.events, "turn.completed", WAIT)
        .await
        .unwrap();
    gate.reached().await;
    let turn = next_turn(common::corpus::thread(), resolved(Some("service")));
    let result = actor
        .handle
        .ask(|reply| Command::StartTurn {
            blocks: conversation::message_blocks(&turn.text, vec![]),
            turn: Box::new(turn),
            reply,
        })
        .await
        .unwrap();
    let error = result.unwrap_err();
    assert_eq!(error.code(), "busy");
    assert!(matches!(
        error,
        orch_agent_contract::adapter::AdapterError::Other {
            retryable: true,
            ..
        }
    ));
    assert_eq!(scripts.launched.lock().unwrap().len(), 1);
    let roster = actor
        .handle
        .ask(|reply| Command::Background { reply })
        .await
        .unwrap();
    assert_eq!(
        roster.items.len(),
        1,
        "the existing exact owned task is retained"
    );
    gate.release();
    let rows = actor.collect().await;
    assert!(
        !rows
            .iter()
            .any(|row| row.name == "turn.started" || row.name == "session.lost")
    );
    assert!(rows.iter().any(|row| row.name == "session.background_tasks"
        && row.body["tasks"][0]["state"] == "completed"));
    actor.close().await;
    let traces = scripts.transcripts();
    assert_eq!(
        traces[0]
            .frames
            .iter()
            .filter(
                |frame| matches!(frame, Frame::ToHarness { frame, .. } if frame["type"] == "user")
            )
            .count(),
        1
    );
    assert!(!traces[0].frames.iter().any(|frame| matches!(frame, Frame::ToHarness { frame, .. } if frame.pointer("/request/subtype") == Some(&json!("stop_task")))));
}

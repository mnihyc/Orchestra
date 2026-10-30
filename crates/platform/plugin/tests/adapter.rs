//! The adapter bridge: the methods of the trait, the events that reach the sink, and one program serving several
//! harness processes, through the scripted plugin.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_agent_contract::adapter::{
    AdapterError, AdapterEvent, AgentAdapter, AgentStart, EventSink, EventStream, PolicyRefusal,
};
use orch_agent_contract::credentials::GivenCredential;
use orch_agent_contract::discovery::{McpImportRequest, McpSearch};
use orch_agent_contract::facts::HarnessState;
use orch_agent_contract::history::ExportKind;
use orch_agent_contract::login::{LoginRequest, LoginValues};
use orch_agent_contract::session::{
    ApprovalDecision, SessionSpec, ToolError, TurnStart, TurnSteer,
};
use orch_plugin::bridges::agent::{AdapterFactory, AgentBridge};
use orch_plugin::supervisor::Plugin;
use orch_plugin_contract::{PluginKind, PluginState};
use orch_types::{Id, Json, ManualClock};
use serde_json::json;
use support::{Work, advance, clock, descriptor, options, registration, script, wait_until};

const PROVIDER: &str = "0199abcd-1234-7000-8000-0000000000a0";
const AGENT_A: &str = "0199abcd-1234-7000-8000-0000000000a1";
const AGENT_B: &str = "0199abcd-1234-7000-8000-0000000000a2";
const THREAD_A: &str = "0199abcd-1234-7000-8000-0000000000c1";
const THREAD_B: &str = "0199abcd-1234-7000-8000-0000000000c2";
const SEGMENT_A: &str = "0199abcd-1234-7000-8000-0000000000d1";
const SEGMENT_B: &str = "0199abcd-1234-7000-8000-0000000000d2";
const TURN: &str = "0199abcd-1234-7000-8000-0000000000e1";

fn id(text: &str) -> Id {
    text.parse().unwrap()
}

/// Every method of an adapter but `turn.steer`, which this adapter's harness cannot do.
const METHODS: [&str; 24] = [
    "agent.start",
    "agent.stop",
    "session.open",
    "session.close",
    "turn.start",
    "turn.interrupt",
    "approval.resolve",
    "orch.tool_result",
    "session.export",
    "session.resync",
    "agent.discover_sessions",
    "mcp.discover",
    "mcp.import",
    "provider.login.start",
    "provider.login.input",
    "provider.login.cancel",
    "provider.logout",
    "provider.refresh",
    "provider.discover_models",
    "provider.credential.issue",
    "provider.credential.accept",
    "agent.credential.return",
    "agent.credential.set",
    "x_extra",
];

fn adapter_script(extra: &Json) -> Json {
    let mut responses = json!({
        "agent.start": { "events": [ { "event": "harness.status", "params": {
                "agent_id": { "$params": "agent_id" }, "status": "online", "detail": null,
                "harness_version": "1.0.0", "sessions_open": 0 } } ],
            "result": { "harness_version": "1.0.0", "descriptor": descriptor(PluginKind::Adapter) } },
        "agent.stop": { "result": {} },
        "session.open": { "events": [ { "event": "session.opened", "params": {
                "thread_id": { "$params": "thread_id" }, "segment_id": { "$params": "segment_id" },
                "native_session_ref": "s-1", "resumed": false } } ],
            "result": { "native_session_ref": "s-1", "resumed": false, "import_report": null } },
        "session.close": { "result": {} },
        "turn.start": {
            "events": [ { "event": "turn.started", "params": {
                "thread_id": THREAD_A, "segment_id": SEGMENT_A, "turn_id": { "$params": "turn_id" },
                "native_turn_ref": "t-1" } } ],
            "result": {},
            "events_after": [ { "event": "turn.completed", "params": {
                "thread_id": THREAD_A, "segment_id": SEGMENT_A, "turn_id": { "$params": "turn_id" },
                "status": "completed" } } ] },
        "turn.interrupt": { "result": {} },
        "approval.resolve": { "result": {} },
        "orch.tool_result": { "result": {} },
        "session.export": { "result": { "format": "orch-neutral-jsonl-v1", "path": "/tmp/x.jsonl", "size": 10 } },
        "session.resync": { "result": { "items": 3 } },
        "agent.discover_sessions": { "result": { "sessions": [ {
            "native_session_ref": "s-9", "title": null, "cwd": "/w", "updated_at": null, "adopted_thread_id": null } ] } },
        "mcp.discover": { "result": { "servers": [ {
            "name": "tmux", "source": "tiny", "scope": "user", "directory": null, "transport": "stdio", "digest": "d1" } ] } },
        "mcp.import": { "result": { "spec": { "transport": "stdio", "command": ["tmux-mcp"] }, "secrets": {} } },
        "provider.login.start": { "result": {} },
        "provider.login.input": { "result": {} },
        "provider.login.cancel": { "result": {} },
        "provider.logout": { "result": {} },
        "provider.refresh": { "result": { "status": "ok", "facts": {
            "plan": "pro", "windows": [], "plan_credits": null, "limit_reached": null,
            "identity": "dana@example.com", "credential": null } } },
        "provider.discover_models": { "result": { "suggestions": [ {
            "model_name": "tiny-1", "display_name": "Tiny 1", "parameters": {}, "listed": true } ] } },
        "provider.credential.issue": { "result": { "form": "api_key", "credential": "sk-issued", "expires_at": null } },
        "provider.credential.accept": { "result": {} },
        "agent.credential.return": { "result": { "form": "api_key", "credential": "sk-returned", "expires_at": null, "upstream": null } },
        "agent.credential.set": { "result": {} }
    });
    if let (Some(responses), Some(extra)) = (responses.as_object_mut(), extra.as_object()) {
        responses.extend(extra.clone());
    }
    script(PluginKind::Adapter, &METHODS, &responses)
}

async fn start(work: &Work, clock: &ManualClock, extra: &Json, args: &[&str]) -> Plugin {
    let plugin = Plugin::start(
        &registration(
            PluginKind::Adapter,
            "tiny",
            work,
            &adapter_script(extra),
            args,
        ),
        options(clock),
    )
    .await
    .unwrap();
    plugin.settled().await;
    assert_eq!(
        plugin.report().state,
        PluginState::Running,
        "{:?}",
        plugin.report().detail
    );
    plugin
}

fn agent_start(agent: &str) -> AgentStart {
    serde_json::from_value(json!({
        "provider_id": PROVIDER, "agent_id": agent, "harness_kind": "tiny",
        "profile": { "harness_version": null, "config": {}, "env": {} }, "home": null, "provider_config": {},
        "loopback": { "base_url": "http://127.0.0.1:41000", "token": "loopback-token" }, "binary": null
    }))
    .unwrap()
}

fn session_spec(agent: &str, thread: &str, segment: &str) -> SessionSpec {
    serde_json::from_value(json!({
        "agent_id": agent, "thread_id": thread, "segment_id": segment, "mode": "new", "native_session_ref": null,
        "upto_turn_ordinal": null, "workspace_root": "/workspace", "instruction_root_marker": false,
        "directory": "/workspace/web", "cwd": "/workspace/web", "mounts": [],
        "model": "tiny-1", "effort": null, "params": {}, "approval_mode": "auto", "sandbox_mode": "open",
        "instructions": "", "memory": null, "arrangement": "together", "orch_tools": [], "raw_events": false,
        "session_key": "0123456789abcdef0123456789abcdef", "archive": null, "history": null,
        "provider": { "path": "native", "provider_id": PROVIDER },
        "exec": { "executor": null, "environment_id": "0199abcd-1234-7000-8000-0000000000f1", "generation": 1 },
        "mcp": []
    }))
    .unwrap()
}

fn turn(thread: &str) -> TurnStart {
    serde_json::from_value(json!({
        "thread_id": thread, "turn_id": TURN, "text": "Fix the redirect.", "images": [], "model": "tiny-1",
        "effort": null, "params": {}, "approval_mode": "auto", "sandbox_mode": "open"
    }))
    .unwrap()
}

/// What one bridge's sink has been given.
struct Heard {
    stream: EventStream,
    seen: Vec<AdapterEvent>,
}

impl Heard {
    fn new() -> (EventSink, Self) {
        let (sink, stream) = EventSink::channel();
        (
            sink,
            Self {
                stream,
                seen: Vec::new(),
            },
        )
    }

    fn names(&mut self) -> Vec<&'static str> {
        while let Ok(event) = self.stream.try_recv() {
            self.seen.push(event);
        }
        self.seen.iter().map(AdapterEvent::name).collect()
    }
}

#[tokio::test]
async fn an_adapter_starts_opens_a_session_runs_a_turn_and_stops_and_hears_what_the_program_emits()
{
    let (clock, work) = (clock(), Work::new());
    let plugin = start(&work, &clock, &json!({}), &[]).await;
    let factory = AdapterFactory::new(plugin.clone()).unwrap();
    assert_eq!(
        (factory.name(), factory.harness_kind().as_deref()),
        ("tiny", Some("tiny"))
    );
    let mut adapter = factory.create().unwrap();
    assert_eq!(adapter.kind(), "tiny");
    assert_eq!(adapter.declared().kind, "tiny");

    let (sink, mut heard) = Heard::new();
    let started = adapter.start(agent_start(AGENT_A), sink).await.unwrap();
    assert_eq!(started.harness_version, "1.0.0");
    assert_eq!(started.descriptor.kind, "tiny");
    assert_eq!(
        heard.names(),
        ["harness.status"],
        "heard before the answer to the start was"
    );
    let sent = &work.received("agent.start")[0]["params"];
    assert_eq!(
        (sent["provider_id"].as_str(), sent["agent_id"].as_str()),
        (Some(PROVIDER), Some(AGENT_A))
    );
    assert_eq!(
        sent["loopback"]["token"], "loopback-token",
        "what the program is given to reach the connector"
    );

    let opened = adapter
        .open_session(session_spec(AGENT_A, THREAD_A, SEGMENT_A))
        .await
        .unwrap();
    assert_eq!(
        (opened.native_session_ref.as_str(), opened.resumed),
        ("s-1", false)
    );
    assert_eq!(
        heard.names(),
        ["harness.status", "session.opened"],
        "the session's first event came before its answer"
    );

    adapter.start_turn(turn(THREAD_A)).await.unwrap();
    wait_until("the turn was reported done", || {
        heard.names().contains(&"turn.completed")
    })
    .await;
    assert_eq!(
        heard.names(),
        [
            "harness.status",
            "session.opened",
            "turn.started",
            "turn.completed"
        ],
        "events reach the sink in the order the program wrote them"
    );
    let AdapterEvent::Session {
        thread_id,
        segment_id,
        ..
    } = &heard.seen[2]
    else {
        panic!("a session event");
    };
    assert_eq!((*thread_id, *segment_id), (id(THREAD_A), id(SEGMENT_A)));

    // What the harness cannot do is not asked of the program, and says which capability it lacks.
    let steer = TurnSteer {
        thread_id: id(THREAD_A),
        turn_id: id(TURN),
        text: "also".into(),
        images: vec![],
    };
    assert_eq!(
        adapter.steer(steer).await,
        Err(AdapterError::Unsupported("steer".into()))
    );
    assert!(work.received("turn.steer").is_empty());

    adapter.interrupt(id(THREAD_A), id(TURN)).await.unwrap();
    assert_eq!(
        work.received("turn.interrupt")[0]["params"]["turn_id"],
        TURN
    );
    let decision = ApprovalDecision::Option {
        option_id: "allow".into(),
        note: None,
    };
    adapter
        .resolve_approval(id(THREAD_A), "req-1", decision)
        .await
        .unwrap();
    assert_eq!(
        work.received("approval.resolve")[0]["params"],
        json!({ "thread_id": THREAD_A, "request_ref": "req-1", "decision": { "option_id": "allow" } })
    );
    adapter
        .tool_result(id(THREAD_A), "call-1", Ok(json!({ "ok": true })))
        .await
        .unwrap();
    adapter
        .tool_result(
            id(THREAD_A),
            "call-2",
            Err(ToolError {
                code: "denied".into(),
                message: "no".into(),
            }),
        )
        .await
        .unwrap();
    let results = work.received("orch.tool_result");
    assert_eq!(results[0]["params"]["result"], json!({ "ok": true }));
    assert_eq!(
        results[1]["params"]["error"],
        json!({ "code": "denied", "message": "no" })
    );
    assert!(results[1]["params"].get("result").is_none());

    let archive = adapter
        .export_session(id(THREAD_A), ExportKind::Neutral, Some(4))
        .await
        .unwrap();
    assert_eq!((archive.path.as_str(), archive.size), ("/tmp/x.jsonl", 10));
    assert_eq!(
        work.received("session.export")[0]["params"]["upto_turn_ordinal"],
        4
    );
    assert_eq!(adapter.resync_session(id(THREAD_A)).await.unwrap(), 3);
    let found = adapter.discover_sessions().await.unwrap();
    assert_eq!(found[0].native_session_ref, "s-9");
    assert_eq!(
        work.received("agent.discover_sessions")[0]["params"],
        json!({ "provider_id": PROVIDER, "agent_id": AGENT_A }),
        "the program is told for which harness process"
    );

    // Asking what the harness is now starts it again, which is how a probe is made.
    assert_eq!(adapter.probe().await.unwrap().kind, "tiny");
    assert_eq!(work.received("agent.start").len(), 2);

    adapter.close_session(id(THREAD_A)).await.unwrap();
    adapter.stop().await.unwrap();
    assert_eq!(
        work.received("agent.stop")[0]["params"],
        json!({ "provider_id": PROVIDER, "agent_id": AGENT_A })
    );
    adapter.stop().await.unwrap();
    assert_eq!(
        work.received("agent.stop").len(),
        1,
        "stopping what is stopped asks nothing"
    );
    plugin.shutdown().await;
}

#[tokio::test]
async fn one_program_serves_several_harness_processes_and_each_hears_only_its_own() {
    let (clock, work) = (clock(), Work::new());
    let plugin = start(&work, &clock, &json!({}), &[]).await;
    let factory = AdapterFactory::new(plugin.clone()).unwrap();
    let (mut first, mut second) = (factory.create().unwrap(), factory.create().unwrap());
    let ((sink_a, mut heard_a), (sink_b, mut heard_b)) = (Heard::new(), Heard::new());

    first.start(agent_start(AGENT_A), sink_a).await.unwrap();
    second.start(agent_start(AGENT_B), sink_b).await.unwrap();
    assert_eq!(
        heard_a.names(),
        ["harness.status"],
        "the status of the second was not for the first"
    );
    assert_eq!(heard_b.names(), ["harness.status"]);

    first
        .open_session(session_spec(AGENT_A, THREAD_A, SEGMENT_A))
        .await
        .unwrap();
    second
        .open_session(session_spec(AGENT_B, THREAD_B, SEGMENT_B))
        .await
        .unwrap();
    first.start_turn(turn(THREAD_A)).await.unwrap();
    wait_until("the first heard its turn end", || {
        heard_a.names().contains(&"turn.completed")
    })
    .await;
    assert_eq!(
        heard_a.names(),
        [
            "harness.status",
            "session.opened",
            "turn.started",
            "turn.completed"
        ]
    );
    assert_eq!(
        heard_b.names(),
        ["harness.status", "session.opened"],
        "a session's events go to the harness process that opened it"
    );

    // A harness process that stopped hears nothing more. The one that is left hears the events of a session that
    // nobody opened there, since there is nobody else to hear them.
    first.stop().await.unwrap();
    let before = heard_a.names().len();
    second.start_turn(turn(THREAD_A)).await.unwrap();
    wait_until("the other one heard the turn", || {
        heard_b.names().contains(&"turn.completed")
    })
    .await;
    assert_eq!(heard_a.names().len(), before);
    plugin.shutdown().await;
}

#[tokio::test]
async fn the_errors_of_an_adapter_keep_their_codes_and_an_adapter_that_was_not_started_says_so() {
    let (clock, work) = (clock(), Work::new());
    let extra = json!({ "turn.start": [
        { "error": { "code": "harness_unavailable", "message": "not installed", "retryable": false } },
        { "error": { "code": "session_not_found", "message": "s-1", "retryable": false } },
        { "error": { "code": "policy.root_not_allowed", "message": "/etc", "retryable": false } },
        { "error": { "code": "vendor.rate_limited", "message": "slow down", "retryable": true } }
    ] });
    let plugin = start(&work, &clock, &extra, &[]).await;
    let mut adapter = AgentBridge::new(plugin.clone()).unwrap();
    assert_eq!(
        adapter.discover_sessions().await,
        Err(AdapterError::HarnessUnavailable(
            "the harness has not been started".into()
        ))
    );
    let (sink, _heard) = Heard::new();
    adapter.start(agent_start(AGENT_A), sink).await.unwrap();
    assert_eq!(
        adapter.start_turn(turn(THREAD_A)).await,
        Err(AdapterError::HarnessUnavailable("not installed".into()))
    );
    assert_eq!(
        adapter.start_turn(turn(THREAD_A)).await,
        Err(AdapterError::SessionNotFound("s-1".into()))
    );
    assert_eq!(
        adapter.start_turn(turn(THREAD_A)).await,
        Err(AdapterError::Policy(PolicyRefusal::RootNotAllowed))
    );
    let own = adapter.start_turn(turn(THREAD_A)).await.unwrap_err();
    assert_eq!((own.code(), own.retryable()), ("vendor.rate_limited", true));
    plugin.shutdown().await;
}

#[tokio::test]
async fn the_provider_side_of_an_adapter_signs_in_reports_its_facts_and_models_and_hands_credentials_over()
 {
    let (clock, work) = (clock(), Work::new());
    let plugin = start(&work, &clock, &json!({}), &[]).await;
    let mut adapter = AgentBridge::new(plugin.clone()).unwrap();
    let (sink, _heard) = Heard::new();
    adapter.start(agent_start(AGENT_A), sink).await.unwrap();
    let who = json!({ "provider_id": PROVIDER, "agent_id": AGENT_A });

    let request: LoginRequest = serde_json::from_value(json!({
        "method": "device_code", "values": { "workspace": "w1" }, "config": { "region": "eu" }
    }))
    .unwrap();
    adapter.login(request).await.unwrap();
    let sent = &work.received("provider.login.start")[0]["params"];
    assert_eq!(
        (
            sent["method"].as_str(),
            sent["values"]["workspace"].as_str()
        ),
        (Some("device_code"), Some("w1"))
    );
    assert_eq!(sent["provider_id"], PROVIDER);
    adapter
        .login_input(serde_json::from_value::<LoginValues>(json!({ "code": "1234" })).unwrap())
        .await
        .unwrap();
    assert_eq!(
        work.received("provider.login.input")[0]["params"]["values"],
        json!({ "code": "1234" })
    );
    adapter.cancel_login().await.unwrap();
    adapter.logout().await.unwrap();
    assert_eq!(work.received("provider.login.cancel")[0]["params"], who);
    assert_eq!(work.received("provider.logout")[0]["params"], who);

    assert_eq!(
        adapter.provider_facts().await.unwrap().identity.as_deref(),
        Some("dana@example.com")
    );
    assert_eq!(
        work.received("provider.refresh")[0]["params"],
        json!({ "provider_id": PROVIDER })
    );
    assert_eq!(
        adapter.discover_models().await.unwrap()[0].model_name,
        "tiny-1"
    );

    let issued = adapter.issue_credential("api_key").await.unwrap();
    assert_eq!(
        (issued.form.as_str(), issued.credential.expose()),
        ("api_key", "sk-issued")
    );
    assert_eq!(
        work.received("provider.credential.issue")[0]["params"],
        json!({ "provider_id": PROVIDER, "form": "api_key" })
    );
    let given: GivenCredential = serde_json::from_value(json!({
        "form": "api_key", "credential": "sk-given", "expires_at": null, "upstream": null
    }))
    .unwrap();
    adapter.set_credential(given.clone()).await.unwrap();
    let sent = &work.received("agent.credential.set")[0]["params"];
    assert_eq!(
        (sent["credential"].as_str(), sent["agent_id"].as_str()),
        (Some("sk-given"), Some(AGENT_A))
    );
    adapter.accept_credential(given).await.unwrap();
    assert_eq!(
        work.received("provider.credential.accept")[0]["params"]["provider_id"],
        PROVIDER
    );
    let returned = adapter.return_credential().await.unwrap();
    assert_eq!(returned.credential.expose(), "sk-returned");

    let found = adapter
        .discover_mcp(McpSearch {
            directories: vec!["/workspace/web".into()],
        })
        .await
        .unwrap();
    assert_eq!(found[0].name, "tmux");
    assert_eq!(
        work.received("mcp.discover")[0]["params"]["directories"][0],
        "/workspace/web"
    );
    let definition = adapter
        .read_mcp(
            serde_json::from_value::<McpImportRequest>(json!({
                "server_id": "0199abcd-1234-7000-8000-0000000000b1", "source": "tiny", "scope": "user",
                "name": "tmux", "directory": null
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(definition.spec.command, Some(vec!["tmux-mcp".to_owned()]));
    plugin.shutdown().await;
}

#[tokio::test]
async fn the_manifest_an_adapter_sends_in_hello_is_its_programs_and_one_that_does_not_fit_is_not() {
    let manifest = |kind: &str| {
        json!({
            "manifest_version": 1, "kind": kind, "names": { "en": "Tiny", "zh-CN": "微小" },
            "protocol": "tiny-protocol", "placement": { "colocated": true, "needs_binary": true }
        })
    };
    let fresh = || (clock(), Work::new());
    let (clock, work) = fresh();
    let mut with = adapter_script(&json!({}));
    with["descriptor"]["manifest"] = manifest("tiny");
    let plugin = Plugin::start(
        &registration(PluginKind::Adapter, "tiny", &work, &with, &[]),
        options(&clock),
    )
    .await
    .unwrap();
    plugin.settled().await;
    let adapter = AgentBridge::new(plugin.clone()).unwrap();
    let sent = adapter.manifest().expect("the manifest it sent");
    assert_eq!(sent.kind, "tiny");
    assert!(sent.placement.needs_binary && sent.placement.colocated);
    assert_eq!(sent.names.get("zh-CN"), "微小");

    // A manifest of another kind than the adapter's own, or one that does not hold together, is nobody's.
    let (clock, work) = fresh();
    let mut other = adapter_script(&json!({}));
    other["descriptor"]["manifest"] = manifest("another");
    let plugin = Plugin::start(
        &registration(PluginKind::Adapter, "tiny", &work, &other, &[]),
        options(&clock),
    )
    .await
    .unwrap();
    plugin.settled().await;
    assert!(AgentBridge::new(plugin).unwrap().manifest().is_none());

    let (clock, work) = fresh();
    let plain = adapter_script(&json!({}));
    let plugin = Plugin::start(
        &registration(PluginKind::Adapter, "tiny", &work, &plain, &[]),
        options(&clock),
    )
    .await
    .unwrap();
    plugin.settled().await;
    assert!(
        AgentBridge::new(plugin).unwrap().manifest().is_none(),
        "an adapter that sends none has none"
    );
}

#[tokio::test]
async fn an_adapter_that_does_not_list_a_method_answers_unsupported_with_the_capability_it_lacks() {
    let (clock, work) = (clock(), Work::new());
    let mut without = adapter_script(&json!({}));
    without["methods"] = json!([
        "agent.start",
        "agent.stop",
        "session.open",
        "session.close",
        "turn.start"
    ]);
    let plugin = Plugin::start(
        &registration(PluginKind::Adapter, "tiny", &work, &without, &[]),
        options(&clock),
    )
    .await
    .unwrap();
    plugin.settled().await;
    let mut adapter = AgentBridge::new(plugin.clone()).unwrap();
    let (sink, _heard) = Heard::new();
    adapter.start(agent_start(AGENT_A), sink).await.unwrap();
    let unsupported = |capability: &str| Err::<(), _>(AdapterError::Unsupported(capability.into()));
    assert_eq!(
        adapter.interrupt(id(THREAD_A), id(TURN)).await,
        unsupported("interrupt")
    );
    assert_eq!(
        adapter
            .resolve_approval(
                id(THREAD_A),
                "r",
                ApprovalDecision::Option {
                    option_id: "o".into(),
                    note: None
                }
            )
            .await,
        unsupported("approvals")
    );
    assert_eq!(
        adapter.tool_result(id(THREAD_A), "c", Ok(json!({}))).await,
        unsupported("orch_tools")
    );
    assert_eq!(adapter.logout().await, unsupported("native_provider"));
    assert_eq!(
        adapter.issue_credential("api_key").await.map(|_| ()),
        unsupported("credentials")
    );
    assert_eq!(
        adapter.discover_models().await.map(|_| ()),
        unsupported("model_discovery")
    );
    assert_eq!(
        adapter.discover_sessions().await.map(|_| ()),
        unsupported("discover_sessions")
    );
    assert_eq!(
        adapter.resync_session(id(THREAD_A)).await.map(|_| ()),
        unsupported("resync")
    );
    assert_eq!(
        adapter
            .export_session(id(THREAD_A), ExportKind::Native, None)
            .await
            .map(|_| ()),
        unsupported("export_native")
    );
    assert_eq!(
        adapter.discover_mcp(McpSearch::default()).await.map(|_| ()),
        unsupported("mcp_discovery")
    );
    for method in [
        "turn.interrupt",
        "provider.logout",
        "mcp.discover",
        "session.resync",
    ] {
        assert!(work.received(method).is_empty(), "{method} was not asked");
    }
    plugin.shutdown().await;
}

#[tokio::test]
async fn events_the_host_does_not_know_are_ignored_and_the_ones_after_them_are_heard() {
    let (clock, work) = (clock(), Work::new());
    let extra = json!({ "turn.interrupt": { "result": {}, "events": [
        { "event": "x_mystery", "params": { "anything": true } },
        { "event": "item.delta", "params": { "thread_id": THREAD_A, "segment_id": SEGMENT_A, "native_ref": 4 } },
        { "event": "turn.started", "params": { "thread_id": THREAD_A, "segment_id": SEGMENT_A, "turn_id": TURN, "native_turn_ref": "t-9" } }
    ] } });
    let plugin = start(&work, &clock, &extra, &[]).await;
    let mut adapter = AgentBridge::new(plugin.clone()).unwrap();
    let (sink, mut heard) = Heard::new();
    adapter.start(agent_start(AGENT_A), sink).await.unwrap();
    adapter
        .open_session(session_spec(AGENT_A, THREAD_A, SEGMENT_A))
        .await
        .unwrap();
    adapter.interrupt(id(THREAD_A), id(TURN)).await.unwrap();
    assert_eq!(
        heard.names(),
        ["harness.status", "session.opened", "turn.started"]
    );
    assert_eq!(
        plugin.report().state,
        PluginState::Running,
        "the program was not stopped for it"
    );
    plugin.shutdown().await;
}

#[tokio::test]
async fn when_the_program_goes_away_every_harness_process_that_was_started_on_it_is_reported_offline()
 {
    let (clock, work) = (clock(), Work::new());
    let marker = work.text("once");
    let plugin = start(
        &work,
        &clock,
        &json!({}),
        &["--exit-on", "die", "--once-marker", &marker],
    )
    .await;
    let factory = AdapterFactory::new(plugin.clone()).unwrap();
    let (mut first, mut second) = (factory.create().unwrap(), factory.create().unwrap());
    let ((sink_a, mut heard_a), (sink_b, mut heard_b)) = (Heard::new(), Heard::new());
    first.start(agent_start(AGENT_A), sink_a).await.unwrap();
    second.start(agent_start(AGENT_B), sink_b).await.unwrap();

    assert!(plugin.call("die", json!({})).await.is_err());
    wait_until("both were told", || {
        heard_a.names().contains(&"harness.status")
            && heard_a.names().len() == 2
            && heard_b.names().len() == 2
    })
    .await;
    for heard in [&heard_a, &heard_b] {
        let AdapterEvent::Status(status) = heard.seen.last().unwrap() else {
            panic!("a status");
        };
        assert_eq!(status.status, HarnessState::Offline);
        assert!(status.detail.as_deref().unwrap().contains("exit status: 3"));
    }
    // While it is down a call is unavailable, and when it is back the harness is started again.
    let down = first.start_turn(turn(THREAD_A)).await.unwrap_err();
    assert_eq!(
        (down.code(), down.retryable()),
        ("unavailable", true),
        "{down:?}"
    );
    wait_until("it waits to be restarted", || {
        plugin.report().retry_at.is_some()
    })
    .await;
    advance(&clock, 1);
    wait_until("it runs again", || {
        plugin.report().state == PluginState::Running
    })
    .await;
    let (sink, mut again) = Heard::new();
    first.start(agent_start(AGENT_A), sink).await.unwrap();
    assert_eq!(again.names(), ["harness.status"]);
    plugin.shutdown().await;
}

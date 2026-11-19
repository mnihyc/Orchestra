//! The two real harnesses working with the connector's `orch` server, each started by the connector from the adapter it carries and run in a container of the development image: Claude Code calling `announce` (the scripted model asks for it, the call is carried to the system as an event of the session and the system's answer is handed back to the harness), and Codex offered the tools of the session.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::docker::{self, ClaudeDocker, CodexDocker, Containers};
use common::model::{MESSAGES, MODEL, Options, ProviderRig, RESPONSES};
use orch_adapter_claude_code::ClaudeCodeAdapter;
use orch_adapter_codex::CodexAdapter;
use orch_connector_core::harnesses::{AdapterRegistration, AdapterRegistry, HomeRule};
use orch_link_contract::commands::{OrchToolResult, SerialKey};
use serde_json::json;

#[derive(Clone, Copy)]
enum Harness {
    Codex,
    Claude,
}

impl Harness {
    fn label(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    fn kind(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude-code",
        }
    }

    fn protocol(self) -> &'static str {
        match self {
            Self::Codex => RESPONSES,
            Self::Claude => MESSAGES,
        }
    }
}

/// The adapter of the harness, started through a launcher that runs it in a container.
fn registry(
    harness: Harness,
    containers: &Arc<Containers>,
    mounts: Vec<std::path::PathBuf>,
) -> AdapterRegistry {
    let mut registry = AdapterRegistry::new();
    let containers = Arc::clone(containers);
    let registration = match harness {
        Harness::Codex => AdapterRegistration {
            kind: orch_adapter_codex::descriptor::KIND,
            adapter_version: orch_adapter_codex::descriptor::ADAPTER_VERSION,
            builtin: true,
            needs_binary: true,
            home_rule: HomeRule::Harness,
            factory: Arc::new(move |clock| {
                Box::new(
                    CodexAdapter::with_launcher(Arc::new(CodexDocker::new(
                        Arc::clone(&containers),
                        mounts.clone(),
                    )))
                    .with_clock(clock),
                )
            }),
        },
        Harness::Claude => AdapterRegistration {
            kind: orch_adapter_claude_code::config::KIND,
            adapter_version: orch_adapter_claude_code::config::ADAPTER_VERSION,
            builtin: true,
            needs_binary: true,
            home_rule: HomeRule::Harness,
            factory: Arc::new(move |clock| {
                Box::new(ClaudeCodeAdapter::with_launcher(
                    clock,
                    Arc::new(ClaudeDocker::new(Arc::clone(&containers), mounts.clone())),
                ))
            }),
        },
    };
    registry.register(registration).unwrap();
    registry
}

/// The tools the system offers a thread that enables all it has built.
const TOOLS: [&str; 3] = ["send_message", "announce", "request_capability"];

/// A connector hosting the real harness in a container, its session opened with the tools of Orch, a lease for the
/// turn; the containers are removed by the caller.
async fn session_of(harness: Harness) -> Option<(ProviderRig, Arc<Containers>, orch_types::Id)> {
    let test = format!("{} working with the orch server", harness.label());
    if !docker::available(&test) {
        return None;
    }
    let containers = Containers::new(harness.label());
    let for_registry = Arc::clone(&containers);
    let options = Options {
        managed: true,
        patience: Some(Duration::from_secs(240)),
        adapters: Some(Box::new(move |data, work| {
            registry(
                harness,
                &for_registry,
                vec![data.to_owned(), work.to_owned()],
            )
        })),
        ..Options::default()
    };
    let p = ProviderRig::build(options, |_, _| ()).await;
    p.start_both_providers().await;
    let home = format!("harness/{}/{}", p.rig.provider, p.rig.agent);
    let start: orch_link_contract::commands::AgentStart = serde_json::from_value(json!({
        "provider_id": p.rig.provider, "agent_id": p.rig.agent, "harness_kind": harness.kind(),
        "profile": { "harness_version": null, "config": {}, "env": {} }, "home": home
    }))
    .unwrap();
    let started = p.rig.succeed(&p.rig.harness_key(), &start).await;
    assert_eq!(started["descriptor"]["kind"], harness.kind());

    // The session is opened with the tools Orch has built, as the system opens one for a thread that enables them.
    let mut body = p
        .rig
        .open_body(p.thread, p.segment, &p.work.path().display().to_string());
    body["model"] = json!(MODEL);
    // These confined test containers use the no-prompt mode, independently of native reviewer trials.
    body["approval_mode"] = json!("bypass");
    body["sandbox_mode"] = json!("open");
    body["arrangement"] = json!("key_separated");
    body["provider"] = json!({
        "path": "forwarded", "provider_id": p.rig.provider, "protocol": harness.protocol(),
        "ticket": { "$ticket": { "kind": "llm" } }
    });
    body["orch_tools"] = json!(TOOLS);
    p.rig.open_session(body).await;
    let lease = p
        .open_lease(p.thread, &[MODEL], Some(harness.protocol()), None)
        .await;
    Some((p, containers, lease))
}

/// What the connector's server lists, by tool name: the text the system declares for each tool.
fn declared() -> std::collections::BTreeMap<String, String> {
    let listing: serde_json::Value =
        serde_json::from_str(include_str!("../src/orch_tools.json")).unwrap();
    listing
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| {
            (
                tool["name"].as_str().unwrap().to_owned(),
                tool["description"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// The tools of the namespace `mcp__orch` in the first request the harness made to the model, by name and
/// description (the Responses format of Codex presents the tools of a server as a namespace).
fn namespaced(p: &ProviderRig) -> std::collections::BTreeMap<String, String> {
    let requests = p.model_requests();
    let namespace = requests
        .iter()
        .flat_map(|request| {
            request.body["tools"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .find(|tool| tool["type"] == "namespace" && tool["name"] == "mcp__orch")
        .unwrap_or_else(|| {
            panic!(
                "the model was offered no namespace mcp__orch: {:?}",
                requests
                    .iter()
                    .map(|request| request.body["tools"]
                        .to_string()
                        .chars()
                        .take(600)
                        .collect::<String>())
                    .collect::<Vec<_>>()
            )
        });
    namespace["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| {
            (
                tool["name"].as_str().unwrap().to_owned(),
                tool["description"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn claude_code_calls_announce_on_the_orch_server_of_the_connector_and_the_systems_answer_is_the_tools_result()
 {
    let Some((p, containers, lease)) = session_of(Harness::Claude).await else {
        return;
    };
    // The model, scripted, asks for the call; Claude Code names a tool of an MCP server `mcp__<server>__<tool>`.
    let script = concat!(
        "Tell the others.\n#!mock\n",
        "tool mcp__orch__announce {\"title\":\"The build moved\",\"body\":\"It is in ./out now\"}\n",
        "then\nsay Told them."
    );
    p.start_turn_in(script, MODEL, "bypass", "open").await;

    // The call reaches the system as an event of the session, with the name and the arguments the model gave.
    p.rig
        .wait_for(p.thread, p.segment, "orch.tool_call", 1)
        .await;
    let call = p
        .rig
        .applied(p.thread, p.segment)
        .into_iter()
        .find(|event| event.kind == "orch.tool_call")
        .unwrap()
        .body;
    assert_eq!(call["tool"], "announce", "{call}");
    assert_eq!(
        call["arguments"],
        json!({ "title": "The build moved", "body": "It is in ./out now" }),
        "{call}"
    );
    assert!(call["call_ref"].as_str().unwrap().starts_with("mcp:"));

    // The system answers it; the harness gets the answer as the result of its tool and the turn goes on to the end.
    let answer = OrchToolResult {
        thread_id: p.thread,
        call_ref: call["call_ref"].as_str().unwrap().to_owned(),
        attempt: None,
        result: Some(json!({ "audience": "topic", "delivered": { "steered": 1 } })),
        error: None,
    };
    p.rig.succeed(&SerialKey::Thread(p.thread), &answer).await;
    p.rig
        .wait_for(p.thread, p.segment, "turn.completed", 1)
        .await;
    let events = p.rig.applied(p.thread, p.segment);
    let completed = events
        .iter()
        .rev()
        .find(|event| event.kind == "turn.completed")
        .unwrap();
    assert_eq!(
        completed.body["status"], "completed",
        "{:?}",
        completed.body
    );

    // The item of the call is the adapter's, written from the harness's own stream: the tool, what the model gave
    // it and what the system answered.
    let item = events
        .iter()
        .filter(|event| event.kind == "item.completed")
        .map(|event| &event.body["payload"]["tool"])
        .find(|tool| tool["tool"] == "announce")
        .unwrap_or_else(|| {
            panic!(
                "no item of the call among {:?}",
                events.iter().map(|event| &event.body).collect::<Vec<_>>()
            )
        });
    assert_eq!(item["kind"], "orch", "{item}");
    assert_eq!(item["arguments"]["title"], "The build moved", "{item}");
    assert_eq!(item["result"]["audience"], "topic", "{item}");

    // The model was offered the three tools of the session, each under the name of its server.
    let requests = p.model_requests();
    let mut names: Vec<String> = requests[0].body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .filter(|name| name.starts_with("mcp__orch__"))
        .map(str::to_owned)
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "mcp__orch__announce",
            "mcp__orch__request_capability",
            "mcp__orch__send_message"
        ]
    );

    // A second answer to the same reference is taken without a failure.
    p.rig.succeed(&SerialKey::Thread(p.thread), &answer).await;
    p.close_lease(lease).await;
    assert!(
        p.rig.controller.violations().is_empty(),
        "{:?}",
        p.rig.controller.violations()
    );
    containers.remove_all();
}

// Codex presents the tools of a server to its model as a namespace (`mcp__orch`) holding functions named as the
// tools are, and calls one by the namespace and the name. The scripted model names no namespace, so this turn
// shows what Codex took from the server (it started it, spoke the protocol to it and listed its tools) and not a
// call.
#[tokio::test]
async fn codex_is_offered_the_tools_of_the_session_from_the_orch_server_of_the_connector_with_the_text_the_system_declares()
 {
    let Some((p, containers, lease)) = session_of(Harness::Codex).await else {
        return;
    };
    p.start_turn_in("hello", MODEL, "bypass", "open").await;
    p.rig
        .wait_for(p.thread, p.segment, "turn.completed", 1)
        .await;
    let offered = namespaced(&p);
    let declared = declared();
    let mut names: Vec<&String> = offered.keys().collect();
    names.sort();
    assert_eq!(names, ["announce", "request_capability", "send_message"]);
    for name in TOOLS {
        assert_eq!(
            offered[name], declared[name],
            "Codex was given the text the system declares for {name}"
        );
    }
    p.close_lease(lease).await;
    containers.remove_all();
}

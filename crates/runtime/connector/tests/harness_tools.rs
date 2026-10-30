//! What a harness on a connector reaches its tools at, end to end: its listener, the tunnel to the machine where the environment is (or no tunnel when the environment is on the same machine), and the executor or the tool server at the end.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod rig_environment;

use std::collections::BTreeMap;

use futures::{SinkExt, StreamExt};
use orch_connector_core::tool_tunnels::ExecScope;
use orch_link_contract::commands::{
    ExecStart, McpStart, McpStartGiven, SandboxMode, Secret, SessionOpen,
};
use orch_link_contract::events::EnvironmentFacts;
use orch_link_contract::session::Role;
use orch_link_contract::tunnels::{Service, TicketSource};
use orch_types::{Id, Json, new_id};
use rig_environment::tunnels::{Harness, messages};
use rig_environment::{EnvRig, PATIENCE, stand_in, stand_in_executors};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

/// The program of the tests when it is run as a tool server: no test at all otherwise.
#[test]
fn stand_in_for_a_tool_server() {
    if std::env::var(stand_in::FLAG).is_ok() {
        stand_in::run();
    }
}

async fn start_executor(rig: &EnvRig, executor: &str) {
    rig.succeed(&ExecStart {
        environment_id: rig.environment,
        executor: executor.to_owned(),
    })
    .await;
}

async fn tool_message<T>(pending: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(PATIENCE, pending)
        .await
        .expect("the harness tools exchange never finished")
}

fn exec_spec(
    rig: &EnvRig,
    source: Id,
    executor: &str,
    tool_set: Option<&str>,
    thread: Id,
    generation: i64,
) -> Json {
    let service = if executor == "orch-exec" {
        Service::Mcp {
            server: "exec".to_owned(),
        }
    } else {
        Service::Exec {
            executor: executor.to_owned(),
        }
    };
    json!({
        "executor": executor, "tool_set": tool_set, "environment_id": rig.environment, "generation": generation,
        "ticket": rig.ticket(TicketSource::Machine(source), service, Some(thread), tool_set)
    })
}

#[tokio::test]
async fn a_harness_reaches_the_executor_of_another_machine_through_its_listener_and_a_tunnel() {
    let mut rig = EnvRig::new();
    rig.parts.executors = vec![
        stand_in_executors::header_echo(),
        stand_in_executors::echo_socket(),
    ];
    rig.config.exec.offers.push("echo-exec".to_owned());
    let workspace = rig.directory("work");
    rig.start().await;
    let facts: EnvironmentFacts = serde_json::from_value(rig.attach(&workspace).await).unwrap();
    start_executor(&rig, "orch-exec").await;
    start_executor(&rig, "echo-exec").await;
    let agent = rig.agent_side().await;
    let thread = new_id();
    let directory = workspace.join("thread").display().to_string();
    let harness = agent
        .open_session(
            thread,
            &directory,
            exec_spec(
                &rig,
                agent.machine_id,
                "orch-exec",
                Some("plain"),
                thread,
                facts.generation,
            ),
            json!([]),
        )
        .await;

    // The harness asks its listener for the server called `exec`, and the executor answers from the other
    // machine, having been given its own token and the session, and nothing of the harness's.
    let (status, kind, body) = harness
        .post(
            &format!("/mcp/{}/exec", harness.key),
            &[],
            &json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/list" }),
        )
        .await;
    assert!(status.is_success(), "{status}: {body}");
    let answer = messages(&kind, &body).remove(0);
    assert_eq!(answer["id"], 5);
    let seen = &answer["result"]["seen"];
    assert_eq!(
        seen["authorization"],
        format!("Bearer {}", stand_in_executors::TOKEN)
    );
    assert!(
        !body.contains(&harness.token),
        "the harness's own token went along: {body}"
    );
    let session = ExecScope::decode(seen["x-orch-session"].as_str().unwrap()).unwrap();
    assert_eq!(session.generation, Some(facts.generation));
    assert_eq!(session.thread_id, thread.to_string());
    assert_eq!(session.tool_set, "plain");
    assert_eq!(session.directory, directory);
    assert_eq!(session.sandbox_mode, SandboxMode::Workspace);
    assert_eq!(answer["result"]["path"], "/mcp");
    assert_eq!(
        seen["x-orch-environment"],
        Json::Null,
        "what the tunnel needed stays in the tunnel"
    );

    // A turn that runs in a stricter sandbox is held to it in the tools: the mode follows the turn.
    let turn = orch_link_contract::commands::TurnStart {
        plan: false,
        thread_id: thread,
        turn_id: new_id(),
        text: "hello".to_owned(),
        images: Vec::new(),
        model: "fake-large".to_owned(),
        effort: None,
        params: serde_json::Map::new(),
        approval_mode: orch_link_contract::commands::ApprovalMode::OnRequest,
        sandbox_mode: SandboxMode::ReadOnly,
        native_action: None,
    };
    agent
        .succeed(
            &orch_link_contract::commands::SerialKey::Thread(thread),
            &turn,
        )
        .await;
    let (_, kind, body) = harness
        .post(
            &format!("/mcp/{}/exec", harness.key),
            &[],
            &json!({ "jsonrpc": "2.0", "id": 6, "method": "tools/list" }),
        )
        .await;
    let again = messages(&kind, &body).remove(0);
    let session =
        ExecScope::decode(again["result"]["seen"]["x-orch-session"].as_str().unwrap()).unwrap();
    assert_eq!(session.sandbox_mode, SandboxMode::ReadOnly);

    // Orch's own tools are served by the agent role itself: an empty request is no message of the protocol and is
    // answered as such. What the listener has no way to reach is said as it is.
    let (status, _, body) = harness
        .post(&format!("/mcp/{}/orch", harness.key), &[], &json!({}))
        .await;
    assert_eq!(status.as_u16(), 200, "{body}");
    assert!(body.to_string().contains("no JSON-RPC message"), "{body}");
    let (status, _, body) = harness
        .post(&format!("/mcp/{}/nothing", harness.key), &[], &json!({}))
        .await;
    assert_eq!(status.as_u16(), 404, "{body}");
    rig.stop().await;
    agent.connector.stop().await;
}

#[tokio::test]
async fn a_harness_opens_a_websocket_to_its_executor_and_its_messages_go_both_ways() {
    let mut rig = EnvRig::new();
    rig.parts.executors = vec![stand_in_executors::echo_socket()];
    rig.config.exec.offers.push("echo-exec".to_owned());
    let workspace = rig.directory("work");
    rig.start().await;
    let facts: EnvironmentFacts = serde_json::from_value(rig.attach(&workspace).await).unwrap();
    start_executor(&rig, "echo-exec").await;
    let agent = rig.agent_side().await;
    let thread = new_id();
    let harness = agent
        .open_session(
            thread,
            &workspace.join("thread").display().to_string(),
            exec_spec(
                &rig,
                agent.machine_id,
                "echo-exec",
                None,
                thread,
                facts.generation,
            ),
            json!([]),
        )
        .await;

    let mut socket = harness.websocket().await.unwrap();
    tool_message(socket.send(Message::text("hello")))
        .await
        .unwrap();
    tool_message(socket.send(Message::binary(vec![9u8, 8, 7])))
        .await
        .unwrap();
    let first = tool_message(socket.next()).await.unwrap().unwrap();
    assert_eq!(first, Message::text("echo hello"));
    let second = tool_message(socket.next()).await.unwrap().unwrap();
    assert_eq!(second, Message::binary(b"echo \x09\x08\x07".to_vec()));
    tool_message(socket.close(None)).await.unwrap();

    // A request that is no WebSocket is told what the address takes.
    let (status, _, _) = harness
        .post(&format!("/exec/{}", harness.key), &[], &json!({}))
        .await;
    assert_eq!(status.as_u16(), 426);
    rig.stop().await;
    agent.connector.stop().await;
}

#[tokio::test]
async fn a_harness_reaches_a_tool_server_of_another_machine_by_its_name() {
    let mut rig = EnvRig::new();
    rig.config.mcp.add = true;
    rig.parts.ports = Some(41000..=41999);
    let workspace = rig.directory("work");
    rig.start().await;
    let facts: EnvironmentFacts = serde_json::from_value(rig.attach(&workspace).await).unwrap();
    let command = new_id();
    let sealed = rig.sealed(command, "from-the-environment");
    let spec = json!({ "transport": "stdio", "command": stand_in::command(),
        "env": { stand_in::FLAG: "1" }, "secret_env": ["STAND_IN_SECRET"], "startup_timeout_secs": 20 });
    rig.send_as(
        command,
        &McpStart::Given(Box::new(McpStartGiven {
            server_id: None,
            environment_id: rig.environment,
            name: "files".to_owned(),
            spec: serde_json::from_value(spec).unwrap(),
            secrets: BTreeMap::from([("STAND_IN_SECRET".to_owned(), Secret::Sealed(sealed))]),
        })),
    )
    .await;
    let agent = rig.agent_side().await;
    let thread = new_id();
    let mcp = json!([{
        "name": "files",
        "ticket": rig.ticket(
            TicketSource::Machine(agent.machine_id),
            Service::Mcp { server: "files".to_owned() },
            Some(thread),
            None
        )
    }]);
    let harness = agent
        .open_session(
            thread,
            &workspace.join("thread").display().to_string(),
            json!({ "executor": null, "environment_id": rig.environment, "generation": facts.generation }),
            mcp,
        )
        .await;
    let (status, kind, body) = harness
        .post(
            &format!("/mcp/{}/files", harness.key),
            &[],
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "secret" } }),
        )
        .await;
    assert!(status.is_success(), "{status}: {body}");
    assert_eq!(
        messages(&kind, &body)[0]["result"]["content"][0]["text"],
        "from-the-environment"
    );
    // A server this session was given no ticket for is not reached.
    let (status, _, _) = harness
        .post(&format!("/mcp/{}/other", harness.key), &[], &json!({}))
        .await;
    assert_eq!(status.as_u16(), 404);
    rig.stop().await;
    agent.connector.stop().await;
}

#[tokio::test]
async fn a_harness_whose_environment_is_on_its_own_machine_reaches_its_tools_without_a_tunnel() {
    let mut rig = EnvRig::with_roles(vec![Role::Agent, Role::Environment]);
    rig.with_the_scripted_harness();
    rig.parts.executors = vec![
        stand_in_executors::header_echo(),
        stand_in_executors::echo_socket(),
    ];
    rig.config.exec.offers.push("echo-exec".to_owned());
    let workspace = rig.directory("work");
    rig.start().await;
    let facts: EnvironmentFacts = serde_json::from_value(rig.attach(&workspace).await).unwrap();
    start_executor(&rig, "orch-exec").await;
    start_executor(&rig, "echo-exec").await;
    let agent_start: orch_link_contract::commands::AgentStart = serde_json::from_value(json!({
        "provider_id": rig.provider, "agent_id": rig.agent, "harness_kind": "fake",
        "profile": { "harness_version": null, "config": {}, "env": {} }, "home": null
    }))
    .unwrap();
    rig.succeed(&agent_start).await;
    let directory = workspace.join("thread").display().to_string();
    for executor in ["orch-exec", "echo-exec"] {
        let thread = new_id();
        // No ticket at all: nothing is opened to the other machine, since there is none.
        let open: SessionOpen = serde_json::from_value(json!({
            "agent_id": rig.agent, "thread_id": thread, "segment_id": new_id(), "mode": "new",
            "native_session_ref": null, "archive": null, "history": null, "upto_turn_ordinal": null,
            "workspace_root": workspace, "instruction_root_marker": false,
            "directory": directory, "cwd": directory, "mounts": [],
            "model": "fake-large", "effort": "medium", "params": {}, "approval_mode": "on_request",
            "sandbox_mode": "workspace", "instructions": "x", "memory": null,
            "arrangement": "separated", "provider": { "path": "native", "provider_id": rig.provider },
            "exec": { "executor": executor, "tool_set": (executor == "orch-exec").then_some("plain"),
                      "environment_id": rig.environment, "generation": facts.generation },
            "mcp": [], "orch_tools": [], "raw_events": false
        }))
        .unwrap();
        rig.succeed(&open).await;
        let connector = rig.connector.as_ref().unwrap();
        let process = connector
            .hosting()
            .unwrap()
            .process(rig.provider, rig.agent)
            .unwrap();
        let harness = Harness {
            port: connector.loopback().port(),
            token: process.grant().token.expose().to_owned(),
            key: process.session_key(thread).unwrap(),
        };
        if executor == "orch-exec" {
            let (status, kind, body) = harness
                .post(
                    &format!("/mcp/{}/exec", harness.key),
                    &[],
                    &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
                )
                .await;
            assert!(status.is_success(), "{status}: {body}");
            let seen = &messages(&kind, &body)[0]["result"]["seen"];
            assert_eq!(
                seen["authorization"],
                format!("Bearer {}", stand_in_executors::TOKEN)
            );
            let session = ExecScope::decode(seen["x-orch-session"].as_str().unwrap()).unwrap();
            assert_eq!(session.generation, Some(facts.generation));
            assert_eq!(
                (session.thread_id, session.tool_set),
                (thread.to_string(), "plain".to_owned())
            );
        } else {
            let mut socket = harness.websocket().await.unwrap();
            tool_message(socket.send(Message::text("near")))
                .await
                .unwrap();
            assert_eq!(
                tool_message(socket.next()).await.unwrap().unwrap(),
                Message::text("echo near")
            );
        }
    }
    rig.stop().await;
}

#[tokio::test]
async fn a_local_harness_cannot_call_a_running_server_it_was_not_given() {
    let mut rig = EnvRig::with_roles(vec![Role::Agent, Role::Environment]);
    rig.with_the_scripted_harness();
    rig.config.mcp.add = true;
    rig.parts.ports = Some(41000..=41999);
    let workspace = rig.directory("work");
    rig.start().await;
    let facts: EnvironmentFacts = serde_json::from_value(rig.attach(&workspace).await).unwrap();
    for (name, value) in [("allowed", "allowed secret"), ("private", "private secret")] {
        let command = new_id();
        let sealed = rig.sealed(command, value);
        let spec = json!({ "transport": "stdio", "command": stand_in::command(),
            "env": { stand_in::FLAG: "1" }, "secret_env": ["STAND_IN_SECRET"], "startup_timeout_secs": 20 });
        let started = rig
            .send_as(
                command,
                &McpStart::Given(Box::new(McpStartGiven {
                    server_id: None,
                    environment_id: rig.environment,
                    name: name.to_owned(),
                    spec: serde_json::from_value(spec).unwrap(),
                    secrets: BTreeMap::from([(
                        "STAND_IN_SECRET".to_owned(),
                        Secret::Sealed(sealed),
                    )]),
                })),
            )
            .await;
        assert!(
            matches!(started, orch_link::commands::Progress::Succeeded(_)),
            "{started:?}"
        );
    }
    let agent_start: orch_link_contract::commands::AgentStart = serde_json::from_value(json!({
        "provider_id": rig.provider, "agent_id": rig.agent, "harness_kind": "fake",
        "profile": { "harness_version": null, "config": {}, "env": {} }, "home": null
    }))
    .unwrap();
    rig.succeed(&agent_start).await;
    let thread = new_id();
    let directory = workspace.join("thread").display().to_string();
    let ticket = rig.ticket(
        TicketSource::Machine(rig.machine_id),
        Service::Mcp {
            server: "allowed".to_owned(),
        },
        Some(thread),
        None,
    );
    let open: SessionOpen = serde_json::from_value(json!({
        "agent_id": rig.agent, "thread_id": thread, "segment_id": new_id(), "mode": "new",
        "native_session_ref": null, "archive": null, "history": null, "upto_turn_ordinal": null,
        "workspace_root": workspace, "instruction_root_marker": false,
        "directory": directory, "cwd": directory, "mounts": [],
        "model": "fake-large", "effort": "medium", "params": {}, "approval_mode": "on_request",
        "sandbox_mode": "workspace", "instructions": "x", "memory": null,
        "arrangement": "separated", "provider": { "path": "native", "provider_id": rig.provider },
        "exec": { "executor": null, "environment_id": rig.environment, "generation": facts.generation },
        "mcp": [{ "name": "allowed", "ticket": ticket }], "orch_tools": [], "raw_events": false
    }))
    .unwrap();
    rig.succeed(&open).await;
    let connector = rig.connector.as_ref().unwrap();
    let process = connector
        .hosting()
        .unwrap()
        .process(rig.provider, rig.agent)
        .unwrap();
    let harness = Harness {
        port: connector.loopback().port(),
        token: process.grant().token.expose().to_owned(),
        key: process.session_key(thread).unwrap(),
    };
    let request = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "secret" } });
    let (status, kind, body) = harness
        .post(&format!("/mcp/{}/allowed", harness.key), &[], &request)
        .await;
    assert!(status.is_success(), "{status}: {body}");
    assert_eq!(
        messages(&kind, &body)[0]["result"]["content"][0]["text"],
        "allowed secret"
    );
    let (status, _, body) = harness
        .post(&format!("/mcp/{}/private", harness.key), &[], &request)
        .await;
    assert_eq!(status.as_u16(), 404, "{body}");
    assert!(!body.contains("private secret"));
    rig.stop().await;
}

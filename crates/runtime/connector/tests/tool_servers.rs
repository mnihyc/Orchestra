//! Tool servers of an environment: started by a command, reached through the tunnel of a session's ticket, filtered as their definition says, killed and started again within their budget, and reported failed past it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod rig_environment;

use std::collections::BTreeMap;
use std::time::Duration;

use orch_link::tunnels::channel::OpenError;
use orch_link_contract::commands::{
    McpStart, McpStartGiven, McpStop, ProcList, ProcSignal, Secret, Signal,
};
use orch_link_contract::tunnels::{OpenErrorCode, Service, TicketSource};
use orch_types::{Id, Json, Time, new_id};
use rig_environment::tunnels::{Opener, messages, post};
use rig_environment::{EnvRig, PATIENCE, stand_in};
use serde_json::json;

/// The program of the tests when it is run as a tool server: no test at all otherwise.
#[test]
fn stand_in_for_a_tool_server() {
    if std::env::var(stand_in::FLAG).is_ok() {
        stand_in::run();
    }
}

fn definition(extra: Json) -> Json {
    let mut spec = json!({
        "transport": "stdio", "command": stand_in::command(), "env": { stand_in::FLAG: "1" },
        "secret_env": ["STAND_IN_SECRET"], "startup_timeout_secs": 20
    });
    for (name, value) in extra.as_object().unwrap() {
        spec[name] = value.clone();
    }
    spec
}

async fn start(rig: &EnvRig, name: &str, spec: Json) -> Json {
    let command = new_id();
    let sealed = rig.sealed(command, "s3cret-value");
    let body = McpStart::Given(Box::new(McpStartGiven {
        server_id: None,
        environment_id: rig.environment,
        name: name.to_owned(),
        spec: serde_json::from_value(spec).unwrap(),
        secrets: BTreeMap::from([("STAND_IN_SECRET".to_owned(), Secret::Sealed(sealed))]),
    }));
    match rig.send_as(command, &body).await {
        orch_link::commands::Progress::Succeeded(result) => result,
        other => panic!("the server was not started: {other:?}"),
    }
}

fn ticket(rig: &EnvRig, opener: &Opener, server: &str, thread: Id) -> String {
    rig.ticket(
        TicketSource::Machine(opener.machine_id),
        Service::Mcp {
            server: server.to_owned(),
        },
        Some(thread),
        None,
    )
}

/// One call to a tool server through a tunnel of its own: the message the server answered with.
async fn call(
    rig: &EnvRig,
    opener: &Opener,
    server: &str,
    request: Json,
) -> Result<Json, OpenError> {
    let tunnel = opener.open(&ticket(rig, opener, server, new_id())).await?;
    let (status, kind, body) = post(tunnel, &[], &request).await;
    assert!(status.is_success(), "{status}: {body}");
    Ok(messages(&kind, &body)
        .into_iter()
        .next()
        .unwrap_or(Json::Null))
}

fn tool(name: &str, arguments: Json) -> Json {
    json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": name, "arguments": arguments } })
}

async fn text_of(
    rig: &EnvRig,
    opener: &Opener,
    server: &str,
    tool_name: &str,
    arguments: Json,
) -> String {
    let answer = call(rig, opener, server, tool(tool_name, arguments))
        .await
        .unwrap();
    answer["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text in {answer}"))
        .to_owned()
}

/// The server's process: its number and the moment it started, as the environment lists processes.
async fn the_servers_process(rig: &EnvRig) -> (i64, Time) {
    let listed = rig
        .succeed(&ProcList {
            environment_id: rig.environment,
        })
        .await;
    let process = listed["processes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|process| {
            process["command"]
                .as_str()
                .is_some_and(|command| command.contains(stand_in::TEST_NAME))
        })
        .unwrap_or_else(|| panic!("the server is not among the processes: {listed}"));
    (
        process["pid"].as_i64().unwrap(),
        serde_json::from_value(process["started_at"].clone()).unwrap(),
    )
}

async fn kill(rig: &EnvRig) {
    let (pid, started_at) = the_servers_process(rig).await;
    rig.succeed(&ProcSignal {
        environment_id: rig.environment,
        pid,
        started_at,
        signal: Signal::Kill,
    })
    .await;
}

/// Calls the server until it answers again, which is after it was started anew.
async fn answers_again(rig: &EnvRig, opener: &Opener, server: &str) {
    let waited = tokio::time::timeout(PATIENCE, async {
        loop {
            if let Ok(answer) = call(
                rig,
                opener,
                server,
                tool("echo", json!({ "text": "again" })),
            )
            .await
                && answer["result"]["content"][0]["text"] == "again"
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "the server never answered again");
}

#[tokio::test]
async fn a_restart_reconstructs_the_saved_tool_server_and_its_sealed_secret_once() {
    use orch_connector_core::roles::RoleHandler;
    let mut rig = EnvRig::new();
    rig.config.mcp.add = true;
    rig.parts.ports = Some(41000..=41999);
    let workspace = rig.directory("recover-mcp");
    rig.start().await;
    rig.attach(&workspace).await;
    start(&rig, "kept", definition(json!({}))).await;
    rig.stop().await;
    rig.start().await;
    let opener = rig.opener().await;
    call(
        &rig,
        &opener,
        "kept",
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}),
    )
    .await
    .unwrap();
    let reply = call(&rig, &opener, "kept",
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"secret","arguments":{}}})).await.unwrap();
    assert!(
        reply.to_string().contains("s3cret-value"),
        "the original sealed command is opened in its original context"
    );
    let role = rig.connector.as_ref().unwrap().environment_role().unwrap();
    let before = role.host().running_servers().await;
    role.recover().await;
    let after = role.host().running_servers().await;
    assert_eq!(before.len(), 1);
    assert_eq!(after.len(), 1);
    assert_eq!(before[0].2.address, after[0].2.address);
    rig.succeed(&McpStop {
        environment_id: rig.environment,
        name: "kept".to_owned(),
    })
    .await;
    rig.stop().await;
    rig.start().await;
    assert!(
        rig.connector
            .as_ref()
            .unwrap()
            .environment_role()
            .unwrap()
            .host()
            .running_servers()
            .await
            .is_empty()
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_tool_server_is_called_through_a_tunnel_with_the_tools_its_definition_offers() {
    let mut rig = EnvRig::new();
    rig.config.mcp.add = true;
    rig.parts.ports = Some(41000..=41999);
    let workspace = rig.directory("work");
    rig.start().await;
    rig.attach(&workspace).await;
    let opener = rig.opener().await;

    let started = start(
        &rig,
        "files",
        definition(json!({ "tools_deny": ["slow", "crash"] })),
    )
    .await;
    assert_eq!(started["service"], "mcp/files");

    let hello = call(
        &rig,
        &opener,
        "files",
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
    )
    .await
    .unwrap();
    assert_eq!(hello["result"]["serverInfo"]["name"], "stand-in");
    let listed = call(
        &rig,
        &opener,
        "files",
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await
    .unwrap();
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["echo", "secret", "whoami"]);
    // Called, it answers; a tool the definition leaves out is not there; the secret reached the server
    // sealed to the machine and nowhere else.
    assert_eq!(
        text_of(&rig, &opener, "files", "echo", json!({ "text": "hello" })).await,
        "hello"
    );
    assert_eq!(
        text_of(&rig, &opener, "files", "secret", json!({})).await,
        "s3cret-value"
    );
    let refused = call(&rig, &opener, "files", tool("crash", json!({})))
        .await
        .unwrap();
    assert_eq!(refused["error"]["code"], -32602);
    // It is among the processes of the environment, and stopped, it is not there to be reached.
    the_servers_process(&rig).await;
    rig.succeed(&McpStop {
        environment_id: rig.environment,
        name: "files".to_owned(),
    })
    .await;
    match opener.open(&ticket(&rig, &opener, "files", new_id())).await {
        Err(OpenError::Refused { code, .. }) => assert_eq!(code, OpenErrorCode::NoSuchService),
        other => panic!("{other:?}"),
    }
    rig.stop().await;
}

#[tokio::test]
async fn a_tool_server_killed_three_times_is_started_again_twice_and_then_reported_failed() {
    let mut rig = EnvRig::new();
    rig.config.mcp.add = true;
    // Two restarts in the window, as a connector is told to allow them: the third death is one too many.
    rig.config.envhost.mcp_restarts = 2;
    rig.parts.ports = Some(41000..=41999);
    let workspace = rig.directory("work");
    rig.start().await;
    rig.attach(&workspace).await;
    let opener = rig.opener().await;
    start(&rig, "flaky", definition(json!({}))).await;
    answers_again(&rig, &opener, "flaky").await;

    let first = text_of(&rig, &opener, "flaky", "whoami", json!({})).await;
    kill(&rig).await;
    answers_again(&rig, &opener, "flaky").await;
    let second = text_of(&rig, &opener, "flaky", "whoami", json!({})).await;
    assert_ne!(first, second, "a new process answers");
    kill(&rig).await;
    answers_again(&rig, &opener, "flaky").await;
    kill(&rig).await;

    // The third death: the server is failed, and a tunnel to it is refused with the reason.
    let refused = tokio::time::timeout(PATIENCE, async {
        loop {
            match opener.open(&ticket(&rig, &opener, "flaky", new_id())).await {
                Err(OpenError::Refused { code, detail }) => return (code, detail),
                _ => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    })
    .await
    .expect("the tunnel to a failed server was never refused");
    assert_eq!(refused.0, OpenErrorCode::Refused);
    assert!(
        refused.1.contains("failed") && refused.1.contains("crashed too often"),
        "{}",
        refused.1
    );
    // The system is told: the environment stays running, and its facts say which server failed and why.
    let said = rig
        .controller
        .wait_until(PATIENCE, |c| {
            c.applied(&rig.stream()).iter().any(|event| {
                event.kind == "env.facts"
                    && event.body["mcp_servers"].as_array().is_some_and(|servers| {
                        servers.iter().any(|server| {
                            server["name"] == "flaky"
                                && server["state"] == "failed"
                                && server["reason"]
                                    .as_str()
                                    .is_some_and(|reason| reason.contains("crashed too often"))
                        })
                    })
            })
        })
        .await;
    assert!(said, "{:?}", rig.applied());
    let status = rig
        .applied()
        .into_iter()
        .rfind(|event| event.kind == "env.status")
        .unwrap();
    assert_eq!(status.body["status"], "running");

    // Started again on purpose, it is a new server with its budget whole.
    start(&rig, "flaky", definition(json!({}))).await;
    answers_again(&rig, &opener, "flaky").await;
    rig.stop().await;
}

#[tokio::test]
async fn a_tool_server_given_the_default_budget_survives_three_deaths_and_not_a_fourth() {
    let mut rig = EnvRig::new();
    rig.config.mcp.add = true;
    rig.parts.ports = Some(41000..=41999);
    let workspace = rig.directory("work");
    rig.start().await;
    rig.attach(&workspace).await;
    let opener = rig.opener().await;
    start(&rig, "sturdy", definition(json!({}))).await;
    for _ in 0..3 {
        answers_again(&rig, &opener, "sturdy").await;
        kill(&rig).await;
    }
    answers_again(&rig, &opener, "sturdy").await;
    kill(&rig).await;
    let refused = tokio::time::timeout(PATIENCE, async {
        loop {
            if let Err(OpenError::Refused { code, .. }) = opener
                .open(&ticket(&rig, &opener, "sturdy", new_id()))
                .await
            {
                return code;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("never refused");
    assert_eq!(refused, OpenErrorCode::Refused);
    rig.stop().await;
}

#[tokio::test]
async fn a_call_that_takes_a_while_is_answered_as_a_stream_that_carries_its_progress() {
    let mut rig = EnvRig::new();
    rig.config.mcp.add = true;
    rig.parts.ports = Some(41000..=41999);
    let workspace = rig.directory("work");
    rig.start().await;
    rig.attach(&workspace).await;
    let opener = rig.opener().await;
    start(&rig, "slow", definition(json!({}))).await;
    let tunnel = opener
        .open(&ticket(&rig, &opener, "slow", new_id()))
        .await
        .unwrap();
    let request = json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/call",
        "params": { "name": "slow", "arguments": {}, "_meta": { "progressToken": "mine" } } });
    let (status, kind, body) = post(tunnel, &[], &request).await;
    assert!(status.is_success(), "{status}");
    assert!(kind.starts_with("text/event-stream"), "{kind}");
    let all = messages(&kind, &body);
    // The progress arrives under the token the client gave, and the answer under its identifier.
    assert!(
        all.iter()
            .any(|message| message["method"] == "notifications/progress"
                && message["params"]["progressToken"] == "mine"),
        "{all:?}"
    );
    let answer = all
        .iter()
        .find(|message| message.get("result").is_some())
        .unwrap();
    assert_eq!(answer["id"], 9);
    assert_eq!(answer["result"]["content"][0]["text"], "done slowly");
    rig.stop().await;
}

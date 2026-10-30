//! Executors of an environment: started by a command, reached through the tunnel of a session's ticket from the machine where the harness runs, with the session held to the environment and to the machine's policy.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod rig_environment;

use std::sync::Arc;

use orch_connector_core::tool_tunnels::{ENVIRONMENT_HEADER, ExecMount, ExecScope, SESSION_HEADER};
use orch_executor_orch::server::{ExecConfig, OrchExec};
use orch_link::tunnels::channel::OpenError;
use orch_link_contract::commands::{ExecStart, ProcList, SandboxMode};
use orch_link_contract::tunnels::{ExecMessage, ExecOpcode, OpenErrorCode, Service, TicketSource};
use orch_types::{Id, Json, new_id};
use rig_environment::tunnels::{Opener, messages, post};
use rig_environment::{EnvRig, PATIENCE, stand_in_executors};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn orch_exec() -> orch_envhost::environment::ExecutorFactory {
    Arc::new(|| {
        Box::new(OrchExec::with_config(ExecConfig {
            ports: Some(41000..=41999),
            ..ExecConfig::default()
        }))
    })
}

fn scope(directory: &std::path::Path) -> ExecScope {
    ExecScope {
        thread_id: "a-thread-the-ticket-overrides".to_owned(),
        tool_set: "claude-code".to_owned(),
        directory: directory.display().to_string(),
        cwd: None,
        sandbox_mode: SandboxMode::Workspace,
        mounts: Vec::new(),
        generation: None,
    }
}

/// One MCP exchange with Orch's own executor through a tunnel of its own.
async fn exchange(
    rig: &EnvRig,
    opener: &Opener,
    thread: Id,
    scope: &ExecScope,
    request: Json,
) -> Result<Vec<Json>, OpenError> {
    let ticket = rig.ticket(
        TicketSource::Machine(opener.machine_id),
        Service::Mcp {
            server: "exec".to_owned(),
        },
        Some(thread),
        Some("plain"),
    );
    let tunnel = opener.open(&ticket).await?;
    let headers = [
        (SESSION_HEADER, scope.encode()),
        (ENVIRONMENT_HEADER, rig.environment.to_string()),
    ];
    let (status, kind, body) = post(tunnel, &headers, &request).await;
    if !status.is_success() {
        return Ok(vec![json!({ "status": status.as_u16(), "text": body })]);
    }
    Ok(messages(&kind, &body))
}

fn call(tool: &str, arguments: Json) -> Json {
    json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": tool, "arguments": arguments } })
}

#[tokio::test]
async fn a_command_is_run_through_orch_exec_over_a_tunnel_with_its_output_and_its_attribution() {
    let mut rig = EnvRig::new();
    rig.parts.executors = vec![orch_exec()];
    let workspace = rig.directory("work");
    rig.start().await;
    rig.attach(&workspace).await;
    let opener = rig.opener().await;
    let started = rig
        .succeed(&ExecStart {
            environment_id: rig.environment,
            executor: "orch-exec".to_owned(),
        })
        .await;
    assert_eq!(started["service"], "mcp/exec");
    assert!(
        started["tool_sets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|set| set == "plain")
    );
    // The executor shows in the facts of the environment, which the system stores.
    rig.wait_for("env.facts", 1).await;

    let thread = new_id();
    let session = scope(&workspace);
    let tools = exchange(
        &rig,
        &opener,
        thread,
        &session,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await
    .unwrap();
    let listed = tools
        .iter()
        .find(|message| message.get("result").is_some())
        .unwrap();
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"shell") && names.contains(&"read"),
        "the plain set: {names:?}"
    );
    assert!(
        !names.contains(&"Bash"),
        "the set is the ticket's, not the other end's: {names:?}"
    );

    // A command runs where the workspace is, and carries the thread the ticket names.
    let ran = exchange(
        &rig,
        &opener,
        thread,
        &session,
        call(
            "shell",
            json!({ "command": "echo here-$ORCH_THREAD_ID; pwd" }),
        ),
    )
    .await
    .unwrap();
    let output = ran.iter().map(ToString::to_string).collect::<String>();
    assert!(output.contains(&format!("here-{thread}")), "{output}");
    assert!(
        output.contains(&workspace.display().to_string()),
        "{output}"
    );

    // What it starts in the background is attributed to the thread among the environment's processes.
    exchange(
        &rig,
        &opener,
        thread,
        &session,
        call(
            "shell",
            json!({ "command": "sleep 30", "background": true }),
        ),
    )
    .await
    .unwrap();
    let processes = rig
        .succeed(&ProcList {
            environment_id: rig.environment,
        })
        .await;
    let attributed = processes["processes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|process| process["thread_id"] == json!(thread))
        .count();
    assert!(
        attributed >= 1,
        "no process is attributed to the thread: {processes}"
    );
    rig.stop().await;
}

#[tokio::test]
async fn orch_exec_is_reached_the_same_when_the_ticket_names_it_as_an_exec_service() {
    let mut rig = EnvRig::new();
    rig.parts.executors = vec![orch_exec()];
    let workspace = rig.directory("work");
    rig.start().await;
    rig.attach(&workspace).await;
    let opener = rig.opener().await;
    rig.succeed(&ExecStart {
        environment_id: rig.environment,
        executor: "orch-exec".to_owned(),
    })
    .await;

    // A ticket of kind `exec` for the executor, as a system that names every executor by its kind signs.
    let thread = new_id();
    let ticket = rig.ticket(
        TicketSource::Machine(opener.machine_id),
        Service::Exec {
            executor: "orch-exec".to_owned(),
        },
        Some(thread),
        Some("plain"),
    );
    let tunnel = opener.open(&ticket).await.unwrap();
    let headers = [
        (SESSION_HEADER, scope(&workspace).encode()),
        (ENVIRONMENT_HEADER, rig.environment.to_string()),
    ];
    let (status, kind, body) = post(
        tunnel,
        &headers,
        &call(
            "shell",
            json!({ "command": "echo over-exec-$ORCH_THREAD_ID" }),
        ),
    )
    .await;
    assert!(status.is_success(), "{status}: {body}");
    let output = messages(&kind, &body)
        .iter()
        .map(ToString::to_string)
        .collect::<String>();
    assert!(output.contains(&format!("over-exec-{thread}")), "{output}");
    rig.stop().await;
}

#[tokio::test]
async fn what_a_session_says_of_itself_is_held_to_the_environment_and_to_the_machines_policy() {
    let mut rig = EnvRig::new();
    rig.parts.executors = vec![orch_exec()];
    let workspace = rig.directory("work");
    let outside = rig.directory("outside");
    std::fs::write(outside.join("private.txt"), "not for the agent").unwrap();
    std::fs::write(workspace.join("inside.txt"), "for the agent").unwrap();
    rig.start().await;
    rig.attach(&workspace).await;
    let opener = rig.opener().await;
    rig.succeed(&ExecStart {
        environment_id: rig.environment,
        executor: "orch-exec".to_owned(),
    })
    .await;
    let thread = new_id();

    // A directory outside the environment's workspace is refused.
    let wrong = exchange(
        &rig,
        &opener,
        thread,
        &scope(&outside),
        call("read", json!({ "path": "private.txt" })),
    )
    .await
    .unwrap();
    assert_eq!(wrong[0]["status"], 403, "{wrong:?}");

    // A mount the environment does not have is refused too.
    let mut with_mount = scope(&workspace);
    with_mount.mounts = vec![ExecMount {
        name: "data".to_owned(),
        path: outside.display().to_string(),
        read_only: false,
    }];
    let unknown = exchange(
        &rig,
        &opener,
        thread,
        &with_mount,
        call("read", json!({ "path": "inside.txt" })),
    )
    .await
    .unwrap();
    assert_eq!(unknown[0]["status"], 403, "{unknown:?}");

    // A session opened for an earlier making of the environment has no tools in this one.
    let mut stale = scope(&workspace);
    stale.generation = Some(999_999);
    let refused = exchange(
        &rig,
        &opener,
        thread,
        &stale,
        call("read", json!({ "path": "inside.txt" })),
    )
    .await
    .unwrap();
    assert_eq!(refused[0]["status"], 403, "{refused:?}");
    assert!(
        refused[0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("made anew"),
        "{refused:?}"
    );

    // A session that asks for no sandbox at all gets the machine's ceiling, `workspace`, and so cannot read
    // what lies beside the workspace.
    let mut open = scope(&workspace);
    open.sandbox_mode = SandboxMode::Open;
    let read = exchange(
        &rig,
        &opener,
        thread,
        &open,
        call(
            "read",
            json!({ "path": outside.join("private.txt").display().to_string() }),
        ),
    )
    .await
    .unwrap();
    let text = read.iter().map(ToString::to_string).collect::<String>();
    assert!(
        !text.contains("not for the agent"),
        "the ceiling did not hold: {text}"
    );
    // What lies inside it can be read.
    let inside = exchange(
        &rig,
        &opener,
        thread,
        &open,
        call("read", json!({ "path": "inside.txt" })),
    )
    .await
    .unwrap();
    assert!(
        inside
            .iter()
            .map(ToString::to_string)
            .collect::<String>()
            .contains("for the agent")
    );

    // Without a session header, the executor is not reached at all.
    let ticket = rig.ticket(
        TicketSource::Machine(opener.machine_id),
        Service::Mcp {
            server: "exec".to_owned(),
        },
        Some(thread),
        Some("plain"),
    );
    let tunnel = opener.open(&ticket).await.unwrap();
    let (status, _, _) = post(tunnel, &[], &call("read", json!({ "path": "inside.txt" }))).await;
    assert_eq!(status.as_u16(), 400);
    rig.stop().await;
}

// ---------------------------------------------------------------------------------------------------------
// An executor of a harness's own protocol

async fn read_message(
    tunnel: &mut orch_link::tunnels::channel::Tunnel,
    inbox: &mut bytes::BytesMut,
) -> ExecMessage {
    let mut chunk = [0u8; 4096];
    tokio::time::timeout(PATIENCE, async {
        loop {
            if let Some(message) = ExecMessage::take(inbox, 1 << 20).unwrap() {
                return message;
            }
            let count = tunnel.read(&mut chunk).await.unwrap();
            assert!(count > 0, "the tunnel ended");
            inbox.extend_from_slice(&chunk[..count]);
        }
    })
    .await
    .expect("no message came")
}

#[tokio::test]
async fn the_messages_of_an_executor_with_a_protocol_of_its_own_go_both_ways_through_an_exec_tunnel()
 {
    let mut rig = EnvRig::new();
    rig.parts.executors = vec![stand_in_executors::echo_socket()];
    rig.config.exec.offers.push("echo-exec".to_owned());
    let workspace = rig.directory("work");
    rig.start().await;
    rig.attach(&workspace).await;
    let opener = rig.opener().await;
    let started = rig
        .succeed(&ExecStart {
            environment_id: rig.environment,
            executor: "echo-exec".to_owned(),
        })
        .await;
    assert_eq!(started["service"], "exec/echo-exec");

    let ticket = rig.ticket(
        TicketSource::Machine(opener.machine_id),
        Service::Exec {
            executor: "echo-exec".to_owned(),
        },
        Some(new_id()),
        None,
    );
    let mut tunnel = opener.open(&ticket).await.unwrap();
    let mut framed = bytes::BytesMut::new();
    ExecMessage {
        opcode: ExecOpcode::Text,
        payload: "one".into(),
    }
    .write(&mut framed)
    .unwrap();
    ExecMessage {
        opcode: ExecOpcode::Binary,
        payload: bytes::Bytes::from_static(&[1, 2, 3]),
    }
    .write(&mut framed)
    .unwrap();
    tunnel.write_all(&framed).await.unwrap();
    let mut inbox = bytes::BytesMut::new();
    let first = read_message(&mut tunnel, &mut inbox).await;
    assert_eq!(
        (first.opcode, first.payload.as_ref()),
        (ExecOpcode::Text, &b"echo one"[..])
    );
    let second = read_message(&mut tunnel, &mut inbox).await;
    assert_eq!(
        (second.opcode, second.payload.as_ref()),
        (ExecOpcode::Binary, &b"echo \x01\x02\x03"[..])
    );

    // An executor that is not started is not reached; one that is stopped is not there.
    let other = rig.ticket(
        TicketSource::Machine(opener.machine_id),
        Service::Exec {
            executor: "no-such-exec".to_owned(),
        },
        Some(new_id()),
        None,
    );
    match opener.open(&other).await {
        Err(OpenError::Refused { code, .. }) => assert_eq!(code, OpenErrorCode::NoSuchService),
        other => panic!("{other:?}"),
    }
    rig.stop().await;
}

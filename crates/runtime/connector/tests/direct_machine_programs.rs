//! A co-located native executor uses the build admitted to its authenticated session.
#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod rig_environment;

use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;

use futures::{SinkExt as _, StreamExt as _};
use orch_adapter_fake::FakeAdapter;
use orch_connector_core::harnesses::{AdapterRegistration, HomeRule};
use orch_link_contract::commands::{AgentStart, ExecStart, SessionClose, SessionOpen};
use orch_link_contract::events::EnvironmentFacts;
use orch_link_contract::session::Role;
use orch_link_contract::tunnels::{Service, Ticket, TicketClaims, TicketSource};
use orch_types::{Id, new_id};
use rig_environment::{EnvRig, PATIENCE, stand_in_executors, tunnels::Harness};
use serde_json::json;
use tokio_tungstenite::tungstenite::{Error, Message};

fn ticket(rig: &EnvRig, thread: Id, version: Option<&str>) -> String {
    let original = rig.ticket(
        TicketSource::Machine(rig.machine_id),
        Service::Exec {
            executor: "echo-exec".into(),
        },
        Some(thread),
        None,
    );
    let mut claims = serde_json::to_value(Ticket::parse(&original).unwrap().claims()).unwrap();
    if let Some(version) = version {
        claims["tool_executor_version"] = json!(version);
    }
    // JSON keeps this deciding control runnable against the producer before the build member exists.
    let claims: TicketClaims = serde_json::from_value(claims).unwrap();
    rig.signer.sign(&claims).unwrap()
}

async fn open(
    rig: &EnvRig,
    workspace: &std::path::Path,
    generation: i64,
    version: Option<&str>,
) -> (Id, Harness) {
    let thread = new_id();
    let directory = workspace.join("thread").display().to_string();
    let open: SessionOpen = serde_json::from_value(json!({
        "agent_id":rig.agent,"thread_id":thread,"segment_id":new_id(),"mode":"new",
        "native_session_ref":null,"archive":null,"history":null,"upto_turn_ordinal":null,
        "workspace_root":workspace,"instruction_root_marker":false,
        "directory":directory,"cwd":directory,"mounts":[],
        "model":"fake-large","effort":"medium","params":{},"approval_mode":"on_request",
        "sandbox_mode":"workspace","instructions":"x","memory":null,
        "arrangement":"separated","provider":{"path":"native","provider_id":rig.provider},
        "exec":{"executor":"echo-exec","environment_id":rig.environment,"generation":generation,
            "ticket":ticket(rig,thread,version)},
        "mcp":[],"orch_tools":[],"output_capture_bytes":67108864, "raw_events":false
    }))
    .unwrap();
    rig.succeed(&open).await;
    let connector = rig.connector.as_ref().unwrap();
    let process = connector
        .hosting()
        .unwrap()
        .process(rig.provider, rig.agent)
        .unwrap();
    (
        thread,
        Harness {
            port: connector.loopback().port(),
            token: process.grant().token.expose().to_owned(),
            key: process.session_key(thread).unwrap(),
        },
    )
}

#[tokio::test]
async fn direct_local_program_channels_require_the_sessions_admitted_build() {
    tokio::time::timeout(PATIENCE, async {
        let mut rig = EnvRig::with_roles(vec![Role::Agent, Role::Environment]);
        rig.adapters
            .register(AdapterRegistration {
                kind: "fake",
                adapter_version: 1,
                builtin: true,
                needs_binary: true,
                home_rule: HomeRule::Connector,
                factory: Arc::new(|clock| Box::new(FakeAdapter::new(clock))),
            })
            .unwrap();
        let program = rig.home.path().join("controlled-program");
        std::fs::write(&program, "#!/bin/sh\nprintf 'controlled 1.0.0\\n'\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        rig.config.agent.harnesses =
            vec![serde_json::from_value(json!({"kind":"fake","path":program})).unwrap()];
        rig.parts.executors = vec![stand_in_executors::local_echo_socket()];
        rig.config.exec.offers.push("echo-exec".into());
        let workspace = rig.directory("work");
        rig.start().await;
        let facts: EnvironmentFacts = serde_json::from_value(rig.attach(&workspace).await).unwrap();
        rig.succeed(&ExecStart {
            environment_id: rig.environment,
            executor: "echo-exec".into(),
        })
        .await;
        let start: AgentStart = serde_json::from_value(json!({
            "provider_id":rig.provider,"agent_id":rig.agent,"harness_kind":"fake",
            "profile":{"harness_version":null,"config":{},"env":{}},"home":null
        }))
        .unwrap();
        rig.succeed(&start).await;
        let (thread, harness) = open(&rig, &workspace, facts.generation, Some("1.0.0")).await;
        let mut socket = harness.websocket().await.unwrap();
        socket
            .send(Message::text("admitted local work"))
            .await
            .unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::text("echo admitted local work")
        );
        socket.close(None).await.unwrap();
        rig.succeed(&SessionClose { thread_id: thread }).await;
        assert!(
            harness.websocket().await.is_err(),
            "closed sessions do not retain local tool access"
        );
        for version in [Some("2.0.0"), None] {
            let (thread, harness) = open(&rig, &workspace, facts.generation, version).await;
            match harness.websocket().await {
                Err(Error::Http(response)) => assert_eq!(response.status().as_u16(), 403),
                other => {
                    panic!("a local channel with build {version:?} was not refused: {other:?}")
                }
            }
            rig.succeed(&SessionClose { thread_id: thread }).await;
        }
        rig.stop().await;
    })
    .await
    .expect("the direct local program control did not complete");
}

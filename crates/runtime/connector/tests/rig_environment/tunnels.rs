//! What the tests of tunnels share: a machine that opens tunnels as a harness's connector does, and an HTTP exchange written into a tunnel.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking; each test uses some of what is here"
)]

use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use orch_link::tunnels::channel::{OpenError, Tunnel};
use orch_link_contract::session::Role;
use orch_testkit::doubles::controller::ScriptedController;
use orch_testkit::doubles::machine::{MachineSpec, ScriptedMachine};
use orch_types::{Id, new_id};
use serde_json::Value;

use super::EnvRig;

/// A machine on the same hub that opens tunnels to the rig's machine.
pub struct Opener {
    pub machine: ScriptedMachine,
    pub machine_id: Id,
    _controller: ScriptedController,
}

impl EnvRig {
    /// A second machine, with the agent role, whose connector opens the tunnels a harness's tools are reached by.
    pub async fn opener(&self) -> Opener {
        let machine_id = new_id();
        let controller = ScriptedController::attach(
            &self.hub,
            Arc::clone(&self.clock),
            machine_id,
            vec![Role::Agent],
        );
        let machine = ScriptedMachine::start(
            &self.hub,
            &self.clock,
            &self.pki.machine(machine_id),
            &MachineSpec {
                roles: vec![Role::Agent],
                ..MachineSpec::default()
            },
        );
        machine.link.connected().await;
        Opener {
            machine,
            machine_id,
            _controller: controller,
        }
    }
}

impl Opener {
    /// Opens a tunnel with a ticket.
    pub async fn open(&self, ticket: &str) -> Result<Tunnel, OpenError> {
        self.machine.link.open_tunnel(ticket).await
    }
}

/// One HTTP exchange over a tunnel: the status, the content type and the body of the answer.
pub async fn post(
    tunnel: Tunnel,
    headers: &[(&str, String)],
    body: &Value,
) -> (StatusCode, String, String) {
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tunnel))
        .await
        .unwrap();
    tokio::spawn(connection);
    let mut request = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("host", "tools")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let request = request
        .body(Full::new(Bytes::from(serde_json::to_vec(body).unwrap())))
        .unwrap();
    let response = tokio::time::timeout(super::PATIENCE, sender.send_request(request))
        .await
        .expect("the answer never came")
        .unwrap();
    let status = response.status();
    let kind = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = tokio::time::timeout(super::PATIENCE, response.into_body().collect())
        .await
        .expect("the answer never ended")
        .unwrap()
        .to_bytes();
    (status, kind, String::from_utf8_lossy(&bytes).into_owned())
}

/// The messages of an answer, whether it came as a JSON body or as a stream of events.
pub fn messages(kind: &str, body: &str) -> Vec<Value> {
    if kind.starts_with("text/event-stream") {
        body.split("\n\n")
            .flat_map(|event| {
                event
                    .lines()
                    .filter_map(|line| line.strip_prefix("data:"))
                    .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
                    .collect::<Vec<_>>()
            })
            .collect()
    } else {
        serde_json::from_str(body).into_iter().collect()
    }
}

// ---------------------------------------------------------------------------------------------------------
// A connector that hosts a harness, and the harness

/// A connector with the agent role on the same hub, hosting the scripted harness, whose sessions reach their
/// tools on the rig's machine.
pub struct AgentSide {
    pub controller: ScriptedController,
    pub connector: orch_connector_core::connector::Connector,
    pub machine_id: Id,
    pub provider: Id,
    pub agent: Id,
    _data: tempfile::TempDir,
}

/// What the harness a connector hosts holds: the address of the listener, its own token, and the key of
/// its session.
#[derive(Debug, Clone)]
pub struct Harness {
    pub port: u16,
    pub token: String,
    pub key: String,
}

impl EnvRig {
    /// Starts a second connector, with the agent role, hosting the scripted harness.
    pub async fn agent_side(&self) -> AgentSide {
        use orch_connector_core::connector::{ConnectorBuilder, ConnectorParams};
        use orch_link_contract::session::{Arch, Os, Platform};
        use orch_pki::enrollment::MachineIdentity;
        use orch_testkit::doubles::link::{Hub, ShapedBlobs};
        let machine_id = new_id();
        let controller = ScriptedController::attach(
            &self.hub,
            Arc::clone(&self.clock),
            machine_id,
            vec![Role::Agent],
        );
        let issued = self.pki.machine(machine_id);
        let identity = MachineIdentity {
            machine_id,
            key: issued.key,
            certificate: issued.certificate.clone(),
            authority: self.pki.certificate(),
        };
        let data = tempfile::tempdir().unwrap();
        let mut config = orch_config::ConnectorConfig::default();
        // The directory this machine's owner offers is the owner's own value, as a file that names it makes it.
        config.environment.roots = vec![self.home.path().display().to_string()];
        config.policy.local_keys = Some(vec!["environment.roots".into()]);
        let mut adapters = orch_connector_core::harnesses::AdapterRegistry::new();
        adapters
            .register(orch_connector_core::harnesses::AdapterRegistration {
                kind: "fake",
                adapter_version: 1,
                builtin: true,
                needs_binary: false,
                home_rule: orch_connector_core::harnesses::HomeRule::Connector,
                factory: Arc::new(|clock| Box::new(orch_adapter_fake::FakeAdapter::new(clock))),
            })
            .unwrap();
        let connector = ConnectorBuilder::new(ConnectorParams {
            follow_system_endpoints: true,
            hosted_endpoints: None,
            version: "0.1.0".to_owned(),
            data_dir: data.path().to_owned(),
            config,
            identity,
            endpoints: vec![Hub::endpoint()],
            roles: vec![Role::Agent],
            managed: false,
            dialer: self.hub.dialer(Some(issued.certificate)),
            clock: Arc::clone(&self.clock),
            platform: Platform {
                os: Os::Linux,
                arch: Arch::X86_64,
            },
            adapters,
            tools_dir: None,
            blobs: Some(Arc::new(ShapedBlobs::new(
                Arc::clone(&self.blobs),
                None,
                i64::MAX,
            ))),
        })
        .start()
        .await
        .unwrap();
        tokio::time::timeout(super::PATIENCE, connector.link().connected())
            .await
            .expect("the agent's connector never got a session");
        AgentSide {
            controller,
            connector,
            machine_id,
            provider: new_id(),
            agent: new_id(),
            _data: data,
        }
    }
}

impl AgentSide {
    /// Sends a command and waits for it; it has to succeed.
    pub async fn succeed<B: orch_link_contract::commands::CommandBody>(
        &self,
        key: &orch_link_contract::commands::SerialKey,
        body: &B,
    ) -> Value {
        let deadline = orch_types::Time::new(chrono::Utc::now() + chrono::Duration::minutes(5));
        let id = self.controller.issue(key, deadline, body);
        match tokio::time::timeout(super::PATIENCE, self.controller.wait_command(id))
            .await
            .expect("the command never ended")
        {
            orch_link::commands::Progress::Succeeded(result) => result,
            other => panic!("the command did not succeed: {other:?}"),
        }
    }

    /// Starts the scripted harness and opens a session of `thread` in `directory` whose tools are the
    /// executor `exec` and the tool servers `mcp` (each with the ticket the system signed for it).
    pub async fn open_session(
        &self,
        thread: Id,
        directory: &str,
        exec: Value,
        mcp: Value,
    ) -> Harness {
        use orch_link_contract::commands::{AgentStart, SerialKey, SessionOpen};
        let start: AgentStart = serde_json::from_value(serde_json::json!({
            "provider_id": self.provider, "agent_id": self.agent, "harness_kind": "fake",
            "profile": { "harness_version": null, "config": {}, "env": {} }, "home": null
        }))
        .unwrap();
        self.succeed(
            &SerialKey::Harness {
                provider_id: self.provider,
                agent_id: self.agent,
            },
            &start,
        )
        .await;
        let open: SessionOpen = serde_json::from_value(serde_json::json!({
            "agent_id": self.agent, "thread_id": thread, "segment_id": new_id(), "mode": "new",
            "native_session_ref": null, "archive": null, "history": null, "upto_turn_ordinal": null,
            "workspace_root": directory, "instruction_root_marker": false,
            "directory": directory, "cwd": directory, "mounts": [],
            "model": "fake-large", "effort": "medium", "params": {}, "approval_mode": "on_request",
            "sandbox_mode": "workspace", "instructions": "x", "memory": null,
            "arrangement": "separated",
            "provider": { "path": "native", "provider_id": self.provider },
            "exec": exec, "mcp": mcp, "orch_tools": [], "raw_events": false
        }))
        .unwrap();
        self.succeed(&SerialKey::Thread(thread), &open).await;
        let hosting = self.connector.hosting().unwrap();
        let process = hosting.process(self.provider, self.agent).unwrap();
        Harness {
            port: self.connector.loopback().port(),
            token: process.grant().token.expose().to_owned(),
            key: process.session_key(thread).unwrap(),
        }
    }
}

impl Harness {
    /// A request to the listener as the harness makes it: its token, and the address it was given.
    pub async fn post(
        &self,
        path: &str,
        headers: &[(&str, String)],
        body: &Value,
    ) -> (StatusCode, String, String) {
        let stream = tokio::time::timeout(
            super::PATIENCE,
            tokio::net::TcpStream::connect(("127.0.0.1", self.port)),
        )
        .await
        .expect("the harness listener never connected")
        .unwrap();
        let (mut sender, connection) = tokio::time::timeout(
            super::PATIENCE,
            hyper::client::conn::http1::handshake(TokioIo::new(stream)),
        )
        .await
        .expect("the harness listener never completed the handshake")
        .unwrap();
        tokio::spawn(connection);
        let mut request = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header("host", format!("127.0.0.1:{}", self.port))
            .header("authorization", format!("Bearer {}", self.token))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        let request = request
            .body(Full::new(Bytes::from(serde_json::to_vec(body).unwrap())))
            .unwrap();
        let response = tokio::time::timeout(super::PATIENCE, sender.send_request(request))
            .await
            .expect("the answer never came")
            .unwrap();
        let status = response.status();
        let kind = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let bytes = tokio::time::timeout(super::PATIENCE, response.into_body().collect())
            .await
            .expect("the answer never ended")
            .unwrap()
            .to_bytes();
        (status, kind, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The WebSocket a harness opens to its executor.
    pub async fn websocket(
        &self,
    ) -> Result<
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        tokio_tungstenite::tungstenite::Error,
    > {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let stream = tokio::time::timeout(
            super::PATIENCE,
            tokio::net::TcpStream::connect(("127.0.0.1", self.port)),
        )
        .await
        .expect("the harness listener never connected")
        .unwrap();
        let mut request = format!("ws://127.0.0.1:{}/exec/{}", self.port, self.key)
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {}", self.token).parse().unwrap(),
        );
        tokio::time::timeout(
            super::PATIENCE,
            tokio_tungstenite::client_async(request, stream),
        )
        .await
        .expect("the harness listener never completed the WebSocket handshake")
        .map(|(socket, _)| socket)
    }
}

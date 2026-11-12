//! A confirmed path replacement retires live local tools through the real connector policy watcher.
#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod rig_environment;

use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;

use bytes::Bytes;
use futures::{SinkExt as _, StreamExt as _};
use http_body_util::{BodyExt as _, Full};
use hyper::{Method, Request};
use hyper_util::rt::TokioIo;
use orch_connector_core::harnesses::{AdapterRegistration, HomeRule};
use orch_executor_contract::executor::ExecutorEndpoint;
use orch_link_contract::commands::{ExecStart, MachineProbe, MachineProgramsCommand};
use orch_link_contract::policy::PolicyKey;
use orch_link_contract::session::{MachineFacts, Role};
use rig_environment::{EnvRig, PATIENCE, stand_in_executors};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest as _, http::HeaderValue},
};

async fn connect(endpoint: &ExecutorEndpoint) -> WebSocketStream<MaybeTlsStream<TcpStream>> {
    let mut request = endpoint.address.as_str().into_client_request().unwrap();
    request.headers_mut().insert(
        "authorization",
        HeaderValue::from_str(&format!(
            "Bearer {}",
            endpoint.token.as_ref().unwrap().expose()
        ))
        .unwrap(),
    );
    tokio_tungstenite::connect_async(request).await.unwrap().0
}

async fn echo_headers(endpoint: &ExecutorEndpoint) -> Value {
    let stream = TcpStream::connect(("127.0.0.1", endpoint.port().unwrap()))
        .await
        .unwrap();
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    let request = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("host", "tools")
        .header("connection", "close")
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!("Bearer {}", endpoint.token.as_ref().unwrap().expose()),
        )
        .body(Full::new(Bytes::from(
            json!({"jsonrpc":"2.0","id":1,"method":"controlled_echo"}).to_string(),
        )))
        .unwrap();
    let exchange = async move {
        let response = sender.send_request(request).await.unwrap();
        assert!(response.status().is_success());
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    };
    let (response, ended) = tokio::join!(exchange, connection);
    ended.unwrap();
    response
}

#[tokio::test]
async fn a_same_version_confirmed_path_change_closes_its_live_channel_and_keeps_unrelated_tools() {
    tokio::time::timeout(PATIENCE, async {
        let mut rig = EnvRig::with_roles(vec![Role::Environment]);
        rig.adapters
            .register(AdapterRegistration {
                kind: "fake",
                adapter_version: 1,
                builtin: true,
                needs_binary: true,
                home_rule: HomeRule::Connector,
                factory: Arc::new(|_| panic!("this environment-only fixture starts no agent")),
            })
            .unwrap();
        let original = rig.home.path().join("original-program");
        let replacement = rig.home.path().join("replacement-program");
        for program in [&original, &replacement] {
            std::fs::write(
                program,
                "#!/bin/sh\n[ \"$1\" = --version ] || exit 1\nprintf 'controlled 1.0.0\\n'\n",
            )
            .unwrap();
            std::fs::set_permissions(program, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        // The existing selection is an Orch value of a handed-over key; every other rig key stays local.
        rig.config.policy.local_keys = Some(
            PolicyKey::ALL
                .iter()
                .filter(|key| {
                    **key != PolicyKey::PolicyDelegated && **key != PolicyKey::AgentHarnesses
                })
                .map(|key| key.as_str().to_owned())
                .collect(),
        );
        rig.config.policy.delegated = vec![PolicyKey::AgentHarnesses.as_str().into()];
        rig.config.policy.values.insert(
            PolicyKey::AgentHarnesses.as_str().into(),
            json!([{"kind":"fake","path":original}]),
        );
        rig.config.exec.offers = vec!["echo-exec".into(), "plain".into()];
        rig.parts.executors = vec![
            stand_in_executors::local_echo_socket(),
            stand_in_executors::header_echo(),
        ];
        let workspace = rig.directory("work");
        rig.start().await;
        let initial: MachineFacts =
            serde_json::from_value(rig.succeed(&MachineProbe {}).await).unwrap();
        let policy = initial.policy.as_ref().unwrap();
        assert!(policy.delegates(PolicyKey::AgentHarnesses));
        assert!(
            !policy
                .local_values
                .contains_key(PolicyKey::AgentHarnesses.as_str())
        );
        assert_eq!(
            initial
                .harnesses
                .iter()
                .find(|program| program.kind == "fake")
                .unwrap()
                .path,
            original.display().to_string()
        );
        rig.attach(&workspace).await;
        for executor in ["echo-exec", "orch-exec"] {
            rig.succeed(&ExecStart {
                environment_id: rig.environment,
                executor: executor.into(),
            })
            .await;
        }
        let host = rig
            .connector
            .as_ref()
            .unwrap()
            .environment_role()
            .unwrap()
            .host()
            .clone();
        let old = host
            .executor_endpoint(rig.environment, "echo-exec")
            .await
            .unwrap();
        let unrelated = host
            .executor_endpoint(rig.environment, "orch-exec")
            .await
            .unwrap();
        let mut channel = connect(&old).await;
        channel.send(Message::text("live old work")).await.unwrap();
        assert_eq!(
            channel.next().await.unwrap().unwrap(),
            Message::text("echo live old work")
        );
        let before = echo_headers(&unrelated).await;
        assert_eq!(
            before["result"]["seen"]["authorization"],
            format!("Bearer {}", stand_in_executors::TOKEN)
        );
        // A real local confirmation changes the policy. No host mutation or stop helper is called here.
        let changed: MachineFacts = serde_json::from_value(
            rig.succeed(&MachineProgramsCommand::ConfirmHarness {
                kind: "fake".into(),
                path: replacement.display().to_string(),
            })
            .await,
        )
        .unwrap();
        let confirmed = changed
            .harnesses
            .iter()
            .find(|program| program.kind == "fake")
            .unwrap();
        assert_eq!(confirmed.path, replacement.display().to_string());
        assert_eq!(confirmed.version, old.version);
        assert!(original.is_file() && replacement.is_file());
        // Keep the old client open until the watcher and executor stop close its accepted connection.
        assert!(matches!(
            channel.next().await,
            None | Some(Err(_)) | Some(Ok(Message::Close(_)))
        ));
        assert!(
            host.executor_endpoint(rig.environment, "echo-exec")
                .await
                .is_none()
        );
        assert_eq!(
            host.executor_endpoint(rig.environment, "orch-exec").await,
            Some(unrelated.clone())
        );
        assert_eq!(echo_headers(&unrelated).await, before);
        drop(channel);
        rig.stop().await;
    })
    .await
    .expect("the policy watcher did not retire only the replaced local executor");
}

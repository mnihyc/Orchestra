//! Retiring a selected program closes its live native channels while unrelated tools keep working.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

use async_trait::async_trait;
use futures::{SinkExt as _, StreamExt as _};
use orch_agent_contract::{adapter::AdapterError, descriptor::SandboxMode};
use orch_envhost::executors::Executors;
use orch_executor_contract::{
    context::EnvironmentContext,
    descriptor::{ExecutorAvailability, ExecutorDescriptor},
    executor::{Executor, ExecutorEndpoint},
};
use orch_plugin_contract::Secret;
use serde_json::json;
use tokio::{
    net::{TcpListener, TcpStream},
    task::{JoinHandle, JoinSet},
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest as _, http::HeaderValue},
};
use tokio_util::sync::CancellationToken;

/// The existing environment-host observer bound covers owned socket cleanup as well.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(15);

/// The stand-in owns both its listener and every accepted connection, like a stopped child does.
struct SocketExecutor {
    kind: &'static str,
    program: bool,
    stop: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl SocketExecutor {
    fn new(kind: &'static str, program: bool) -> Self {
        Self {
            kind,
            program,
            stop: CancellationToken::new(),
            task: None,
        }
    }
}

impl Drop for SocketExecutor {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[allow(
    clippy::result_large_err,
    reason = "the callback has the WebSocket library's required response type"
)]
fn authenticate(
    request: &tokio_tungstenite::tungstenite::handshake::server::Request,
    response: tokio_tungstenite::tungstenite::handshake::server::Response,
    token: &str,
) -> Result<
    tokio_tungstenite::tungstenite::handshake::server::Response,
    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
> {
    if request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        == Some(&format!("Bearer {token}"))
    {
        Ok(response)
    } else {
        Err(tokio_tungstenite::tungstenite::http::Response::builder()
            .status(401)
            .body(None)
            .unwrap())
    }
}

async fn listener() -> TcpListener {
    // Keep every new listener in the task's configured range; the search is bounded by its ports.
    for port in 41000..=41999 {
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)).await {
            return listener;
        }
    }
    panic!("no owned test port is available");
}

#[async_trait]
impl Executor for SocketExecutor {
    fn kind(&self) -> &'static str {
        self.kind
    }
    fn describe(&self) -> ExecutorDescriptor {
        let mut value = json!({"kind":self.kind,"title":"Controlled native socket","protocol":"harness","sandbox_modes":["open"]});
        if self.program {
            value["program"] = json!("codex");
        }
        serde_json::from_value(value).unwrap()
    }
    async fn available(&self, environment: &EnvironmentContext) -> ExecutorAvailability {
        if self.program
            && !environment
                .local_programs
                .get("codex")
                .is_some_and(|program| std::path::Path::new(program).is_file())
        {
            return ExecutorAvailability::unavailable("the controlled program is not selected");
        }
        ExecutorAvailability::available(
            environment
                .variables
                .get("CONTROLLED_VERSION")
                .map_or("1.0.0", String::as_str),
            vec![SandboxMode::Open],
        )
    }
    async fn start(
        &mut self,
        environment: &EnvironmentContext,
    ) -> Result<ExecutorEndpoint, AdapterError> {
        self.stop = CancellationToken::new();
        let stop = self.stop.clone();
        let listener = listener().await;
        let port = listener.local_addr().unwrap().port();
        let token = orch_types::new_id().to_string();
        let expected = token.clone();
        self.task = Some(tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    () = stop.cancelled() => break,
                    accepted = listener.accept() => {
                        let Ok((stream,_)) = accepted else { break; };
                        let token = expected.clone();
                        connections.spawn(async move {
                            let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(stream,
                                move |request: &_,response|authenticate(request,response,&token)).await else { return; };
                            while let Some(Ok(message)) = socket.next().await {
                                if socket.send(message).await.is_err() { return; }
                            }
                        });
                    },
                    _ = connections.join_next(), if !connections.is_empty() => {}
                }
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        }));
        Ok(ExecutorEndpoint {
            address: format!("ws://127.0.0.1:{port}"),
            token: Some(Secret::new(token)),
            version: environment
                .variables
                .get("CONTROLLED_VERSION")
                .map_or("1.0.0", String::as_str)
                .into(),
            tool_sets: Vec::new(),
            sandbox_modes: vec![SandboxMode::Open],
        })
    }
    async fn stop(&mut self) -> Result<(), AdapterError> {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        Ok(())
    }
}

async fn connect(endpoint: &ExecutorEndpoint) -> WebSocketStream<TcpStream> {
    let mut request = endpoint.address.clone().into_client_request().unwrap();
    request.headers_mut().insert(
        "authorization",
        HeaderValue::from_str(&format!(
            "Bearer {}",
            endpoint.token.as_ref().unwrap().expose()
        ))
        .unwrap(),
    );
    let stream = TcpStream::connect(("127.0.0.1", endpoint.port().unwrap()))
        .await
        .unwrap();
    tokio_tungstenite::client_async(request, stream)
        .await
        .unwrap()
        .0
}

fn environment(directory: &std::path::Path, program: &std::path::Path) -> EnvironmentContext {
    serde_json::from_value(json!({
        "environment_id":orch_types::new_id(),"generation":1,"data_dir":directory,
        "workspace_root":directory,"home":null,"tools_dir":null,"tools":[],
        "local_programs":{"codex":program},"mounts":[],"user":"agent",
        "shell":{"name":"sh","path":"/bin/sh"},"os":"linux","arch":"x86_64","path_style":"posix",
        "limits":{},"variables":{},"mcp_servers_enabled":false
    }))
    .unwrap()
}

#[tokio::test]
async fn replacing_a_same_build_path_closes_old_native_channels_before_new_work_is_admitted() {
    tokio::time::timeout(PATIENCE, async {
        let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let old = directory.path().join("old-program");
        let new = directory.path().join("new-program");
        std::fs::write(&old, "controlled old file").unwrap();
        std::fs::write(&new, "controlled new file").unwrap();
        let mut environment = environment(directory.path(), &old);
        let executors = Executors::new();
        executors
            .register(Box::new(SocketExecutor::new("local-program", true)))
            .await
            .unwrap();
        executors
            .register(Box::new(SocketExecutor::new("other-tools", false)))
            .await
            .unwrap();
        executors
            .start("local-program", None, &environment)
            .await
            .unwrap();
        executors
            .start("other-tools", None, &environment)
            .await
            .unwrap();
        let old_endpoint = executors.endpoint("local-program").await.unwrap();
        let other_endpoint = executors.endpoint("other-tools").await.unwrap();
        let mut old_channel = connect(&old_endpoint).await;
        let mut other_channel = connect(&other_endpoint).await;
        old_channel
            .send(Message::text("active old work"))
            .await
            .unwrap();
        assert_eq!(
            old_channel.next().await.unwrap().unwrap(),
            Message::text("active old work")
        );
        environment
            .local_programs
            .insert("codex".into(), new.to_string_lossy().into_owned());
        executors
            .stop_changed_programs(&environment.local_programs, &environment)
            .await;
        assert!(executors.endpoint("local-program").await.is_none());
        // The current task never cooperated with stop or ended its own native conversation.
        let ended = old_channel.next().await;
        assert!(matches!(
            ended,
            None | Some(Err(_)) | Some(Ok(Message::Close(_)))
        ));
        other_channel
            .send(Message::text("unrelated work survives"))
            .await
            .unwrap();
        assert_eq!(
            other_channel.next().await.unwrap().unwrap(),
            Message::text("unrelated work survives")
        );
        assert!(
            executors
                .ensure_at_version(
                    "local-program",
                    None,
                    &environment,
                    None,
                    Some(&old_endpoint)
                )
                .await
                .is_err()
        );
        let replacement = executors
            .ensure_at_version("local-program", None, &environment, Some("1.0.0"), None)
            .await
            .unwrap();
        assert_ne!(replacement, old_endpoint);
        let mut new_channel = connect(&replacement).await;
        new_channel
            .send(Message::text("new admitted work"))
            .await
            .unwrap();
        assert_eq!(
            new_channel.next().await.unwrap().unwrap(),
            Message::text("new admitted work")
        );
        executors.stop_all().await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn current_program_availability_does_not_prove_an_older_running_endpoint_matches_the_ticket()
{
    tokio::time::timeout(PATIENCE, async {
        for expected in ["2.0.0", "unknown"] {
            let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
            let program = directory.path().join("controlled-program");
            std::fs::write(&program, "controlled file").unwrap();
            let mut environment = environment(directory.path(), &program);
            environment.tools_dir = Some(directory.path().to_string_lossy().into_owned());
            let executors = Executors::new();
            executors
                .register(Box::new(SocketExecutor::new("local-program", true)))
                .await
                .unwrap();
            executors
                .start("local-program", None, &environment)
                .await
                .unwrap();
            let original = executors.endpoint("local-program").await.unwrap();
            let mut channel = connect(&original).await;
            environment.tools_dir = None;
            environment
                .variables
                .insert("CONTROLLED_VERSION".into(), expected.into());
            assert!(
                executors
                    .ensure_at_version("local-program", None, &environment, Some(expected), None)
                    .await
                    .is_err()
            );
            assert_eq!(executors.endpoint("local-program").await, Some(original));
            channel
                .send(Message::text(
                    "the existing endpoint was not retired as a side effect",
                ))
                .await
                .unwrap();
            assert_eq!(
                channel.next().await.unwrap().unwrap(),
                Message::text("the existing endpoint was not retired as a side effect")
            );
            executors.stop_all().await;
        }
    })
    .await
    .unwrap();
}

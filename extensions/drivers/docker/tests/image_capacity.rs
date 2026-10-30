//! A full host must not disguise an image pull failure as an offline registry.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![cfg(unix)]

use std::sync::{Arc, Mutex};

use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_docker::{DockerDriver, config::DockerConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;

/// An owned mock daemon whose socket and task cannot affect a real Docker daemon.
struct Daemon {
    _directory: tempfile::TempDir,
    task: tokio::task::JoinHandle<()>,
    requests: Arc<Mutex<Vec<String>>>,
    socket: String,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Daemon {
    async fn start(stream_error: bool, message: &'static str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("d.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut header = Vec::new();
                loop {
                    let mut byte = [0];
                    if socket.read(&mut byte).await.unwrap_or(0) == 0 {
                        break;
                    }
                    header.push(byte[0]);
                    if header.ends_with(b"\r\n\r\n") || header.len() > 32 * 1024 {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&header);
                let first = request.lines().next().unwrap_or_default().to_owned();
                let length = request.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                });
                if let Some(length) = length {
                    let mut body = vec![0; length];
                    let _ = socket.read_exact(&mut body).await;
                } else if request.lines().any(|line| {
                    line.split_once(':').is_some_and(|(name, value)| {
                        name.eq_ignore_ascii_case("transfer-encoding")
                            && value.trim().eq_ignore_ascii_case("chunked")
                    })
                }) {
                    let mut body = Vec::new();
                    while !body.ends_with(b"\r\n0\r\n\r\n") && body.len() < 1024 * 1024 {
                        let mut chunk = [0; 4096];
                        let read = socket.read(&mut chunk).await.unwrap_or(0);
                        if read == 0 {
                            break;
                        }
                        body.extend_from_slice(&chunk[..read]);
                    }
                }
                observed.lock().unwrap().push(first.clone());
                let (status, body) = if first.contains("/version") {
                    (200, r#"{"ApiVersion":"1.44","Version":"27.0"}"#.to_owned())
                } else if first.contains("/info") {
                    (200, "{}".to_owned())
                } else if first.contains("/images/create?") {
                    if stream_error {
                        (
                            200,
                            serde_json::json!({"error":message,"errorDetail":{"message":message}})
                                .to_string()
                                + "\n",
                        )
                    } else {
                        (500, serde_json::json!({"message":message}).to_string())
                    }
                } else if first.contains("/build?") {
                    (
                        200,
                        format!(
                            "{}\n{}\n",
                            serde_json::json!({"stream":"building a layer\n"}),
                            serde_json::json!({"error":message,"errorDetail":{"message":message}})
                        ),
                    )
                } else if first.contains("/images/") && first.contains("orch-build") {
                    (404, r#"{"message":"no built image"}"#.to_owned())
                } else if first.contains("/images/") && first.contains("/json") {
                    (200, r#"{"Id":"sha256:1234","RepoDigests":["registry.example.test/team/base@sha256:1234"]}"#.to_owned())
                } else {
                    (
                        500,
                        r#"{"message":"the controlled daemon has no such operation"}"#.to_owned(),
                    )
                };
                let response = format!(
                    "HTTP/1.1 {status} fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        Self {
            _directory: directory,
            task,
            requests,
            socket: path.to_string_lossy().into_owned(),
        }
    }

    async fn driver(&self) -> DockerDriver {
        let mut config = DockerConfig {
            socket: self.socket.clone(),
            ..DockerConfig::default()
        };
        config
            .registries
            .push(orch_driver_docker::config::RegistryAuth {
                server: "registry.example.test".into(),
                username: Some("fixture".into()),
                password: Some("fixture-password".into()),
                identity_token: None,
            });
        DockerDriver::connect_for_inventory(config).await.unwrap()
    }
}

#[tokio::test]
async fn a_full_disk_fails_a_pull_instead_of_falling_back_to_an_old_cached_image() {
    for stream_error in [false, true] {
        let daemon = Daemon::start(
            stream_error,
            "failed to unpack: no space left on device; fixture-password",
        )
        .await;
        let driver = daemon.driver().await;
        let result = driver
            .ensure_image(&serde_json::json!({"image":"registry.example.test/team/base:latest"}))
            .await;
        assert!(
            matches!(&result, Err(DriverError::NoCapacity(reason)) if reason.contains("no space left") && reason.contains("<hidden>") && !reason.contains("fixture-password")),
            "the disk failure must be visible and credentials hidden: {result:?}"
        );
        let requests = daemon.requests.lock().unwrap();
        assert!(requests.iter().any(|line| line.contains("/images/create?")));
        assert!(
            !requests
                .iter()
                .any(|line| line.contains("/containers/create?")),
            "a failed pull must not probe or use the cached image"
        );
    }
}

#[tokio::test]
async fn an_unreachable_registry_can_still_use_the_cached_tag() {
    let daemon = Daemon::start(true, "registry is unreachable").await;
    let driver = daemon.driver().await;
    let image = driver
        .ensure_image(&serde_json::json!({"image":"registry.example.test/team/base:latest"}))
        .await
        .unwrap();
    assert_eq!(image.digest.as_deref(), Some("sha256:1234"));
}

#[tokio::test]
async fn a_full_disk_during_a_build_reports_capacity_and_keeps_its_log() {
    let daemon = Daemon::start(true, "no space left on device").await;
    let driver = daemon.driver().await;
    let result = driver
        .ensure_image(&serde_json::json!({"build":{
            "dockerfile":"FROM scratch\nRUN echo work", "context":null, "args":{}, "target":null
        }}))
        .await;
    assert!(
        matches!(&result, Err(DriverError::NoCapacity(reason)) if reason.contains("no space left") && reason.contains("building a layer")),
        "the full disk and build log must remain visible: {result:?}"
    );
    assert!(
        daemon
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains("/build?"))
    );
}

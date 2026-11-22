//! A scripted cached build runs only inside the node's disposable networkless conformance unit.
#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use orch_driver_docker::DockerConfig;
use orch_link_contract::commands::NodeToolConformance;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn a_scripted_build_cannot_certify_another_version_and_its_unit_is_removed() {
    check(Some("orch-dev-harness")).await;
}

#[tokio::test]
async fn a_fresh_host_prepares_its_check_image_and_runs_isolated_cases_without_a_preload_setting() {
    check(None).await;
}

fn conformance_runner() -> std::path::PathBuf {
    std::env::var_os("ORCH_TEST_CONFORMANCE_NODE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_orch-node").into())
}

async fn check(image: Option<&str>) {
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    docker
        .inspect_image("orch-dev-harness")
        .await
        .expect("the pinned harness image is required");
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let program = directory.path().join("codex");
    std::fs::write(
        &program,
        r#"#!/usr/bin/env node
const interfaces = Object.values(require('os').networkInterfaces()).flat();
if (interfaces.some(address => address && !address.internal)) throw new Error('external network is present');
if (process.getuid() === 0) throw new Error('the check runs as root');
if (process.env.HOME !== process.env.CODEX_HOME) throw new Error('the check does not have its own native home');
const readline = require('readline').createInterface({input: process.stdin});
readline.on('line', line => {
  const request = JSON.parse(line);
  if (request.id === undefined) return;
  const result = request.method === 'initialize'
    ? {userAgent: 'orch/0.170.0', codexHome: process.env.CODEX_HOME}
    : {};
  process.stdout.write(JSON.stringify({id: request.id, result}) + '\n');
});
"#,
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let config = DockerConfig {
        labels: std::collections::BTreeMap::from([(
            "orch.conformance_test".to_owned(),
            directory.path().display().to_string(),
        )]),
        live_helper_binary: Some(conformance_runner()),
        conformance_image: image.map(str::to_owned),
        ..Default::default()
    };
    let request = NodeToolConformance {
        name: "codex".to_owned(),
        version: "0.171.0".to_owned(),
        programs: vec!["codex".to_owned()],
        case_timeout_secs: 5,
        timeout_secs: if image.is_some() { 60 } else { 1_800 },
    };
    let began = std::time::Instant::now();
    let result = orch_node::conformance_unit::run(
        &docker,
        &config,
        &request,
        &program,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        result.cases.len(),
        orch_agent_programs::Programs::builtin()
            .program("codex")
            .unwrap()
            .checks
            .cases()
            .count()
    );
    assert!(
        result.cases.iter().all(|case| !case.passed
            && case
                .error
                .as_ref()
                .is_some_and(|error| error.contains("0.170.0") && error.contains("0.171.0"))),
        "{result:?}"
    );
    assert!(began.elapsed() < Duration::from_secs(u64::from(request.timeout_secs)));
    let units = docker
        .list_containers(Some(
            bollard::query_parameters::ListContainersOptionsBuilder::default()
                .all(true)
                .filters(&std::collections::HashMap::from([(
                    "label".to_owned(),
                    vec![format!(
                        "orch.conformance_test={}",
                        directory.path().display()
                    )],
                )]))
                .build(),
        ))
        .await
        .unwrap();
    assert!(units.is_empty(), "the runner leaves no owned unit behind");
}

/// A daemon that, like the host's Docker of a node in a container, has the node's data directory at the same path and
/// none of the container's own files: a bind it cannot find is refused in Docker's own words. It records every unit
/// it is asked to make and answers a unit's run with an empty report.
async fn host_daemon(
    host_has: std::path::PathBuf,
) -> (
    bollard::Docker,
    std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    tokio::task::JoinHandle<()>,
) {
    use axum::Router;
    use axum::body::Bytes;
    use axum::http::{Method, StatusCode, Uri, header};
    use axum::response::IntoResponse;
    let made = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = std::sync::Arc::clone(&made);
    let app = Router::new().fallback(move |method: Method, uri: Uri, body: Bytes| {
        let seen = std::sync::Arc::clone(&seen);
        let host_has = host_has.clone();
        async move {
            let path = uri.path().to_owned();
            if method == Method::GET && path.contains("/images/") {
                return axum::Json(serde_json::json!({"Id": format!("sha256:{}", "a".repeat(64))}))
                    .into_response();
            }
            if method == Method::POST && path.ends_with("/containers/create") {
                let unit: serde_json::Value = serde_json::from_slice(&body).unwrap();
                seen.lock().unwrap().push(unit.clone());
                let mounts = unit["HostConfig"]["Mounts"].as_array().cloned().unwrap_or_default();
                if let Some(missing) = mounts
                    .iter()
                    .filter_map(|mount| mount["Source"].as_str())
                    .find(|source| !std::path::Path::new(source).starts_with(&host_has))
                {
                    return (
                        StatusCode::BAD_REQUEST,
                        axum::Json(serde_json::json!({"message": format!(
                            "invalid mount config for type \"bind\": bind source path does not exist: {missing}"
                        )})),
                    )
                        .into_response();
                }
                return (
                    StatusCode::CREATED,
                    axum::Json(serde_json::json!({"Id": "unit", "Warnings": []})),
                )
                    .into_response();
            }
            if method == Method::POST && path.ends_with("/start") {
                return StatusCode::NO_CONTENT.into_response();
            }
            if method == Method::POST && path.ends_with("/wait") {
                return axum::Json(serde_json::json!({"StatusCode": 0})).into_response();
            }
            if method == Method::GET && path.ends_with("/logs") {
                let report = br#"{"cases":[]}"#;
                let mut frame = vec![1, 0, 0, 0];
                frame.extend_from_slice(&u32::try_from(report.len()).unwrap().to_be_bytes());
                frame.extend_from_slice(report);
                return (
                    [(header::CONTENT_TYPE, "application/vnd.docker.multiplexed-stream")],
                    frame,
                )
                    .into_response();
            }
            if method == Method::DELETE {
                return StatusCode::NO_CONTENT.into_response();
            }
            (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({"message": path}))).into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let docker = bollard::Docker::connect_with_http(
        &format!("http://{address}"),
        10,
        bollard::API_DEFAULT_VERSION,
    )
    .unwrap();
    (docker, made, task)
}

#[tokio::test]
async fn a_bundled_node_whose_own_program_its_daemon_cannot_see_runs_the_check_from_its_data_directory()
 {
    // The bundled node runs in a container from the release mounted at /opt/orch; the host's Docker resolves every
    // bind on the host, where that path does not exist. Its data directory is mounted at the path it has there.
    let data = tempfile::tempdir().unwrap();
    let container = tempfile::tempdir().unwrap();
    let runner = container.path().join("orch-node");
    std::fs::copy(conformance_runner(), &runner).unwrap();
    let mut settings = orch_node_core::settings::NodeSettings::in_dir(data.path());
    settings.docker.live_helper_binary = Some(runner.clone());
    settings.docker.conformance_image = Some("checks.example/native:ready".to_owned());
    let config = orch_node::docker::config_of(&settings).unwrap();
    let build = settings.tools_dir().join("codex").join("0.171.0");
    std::fs::create_dir_all(&build).unwrap();
    let program = build.join("codex");
    std::fs::write(&program, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (docker, made, task) = host_daemon(data.path().to_owned()).await;
    let request = NodeToolConformance {
        name: "codex".to_owned(),
        version: "0.171.0".to_owned(),
        programs: vec!["codex".to_owned()],
        case_timeout_secs: 5,
        timeout_secs: 60,
    };
    let result = orch_node::conformance_unit::run(
        &docker,
        &config,
        &request,
        &program,
        &CancellationToken::new(),
    )
    .await;
    task.abort();
    assert_eq!(result.unwrap().cases, Vec::new());
    let units = made.lock().unwrap().clone();
    assert_eq!(units.len(), 1, "one unit is made");
    let mounts = units[0]["HostConfig"]["Mounts"].as_array().unwrap();
    let source = |target: &str| {
        mounts
            .iter()
            .find(|mount| mount["Target"] == target)
            .and_then(|mount| mount["Source"].as_str())
            .map(std::path::PathBuf::from)
            .unwrap()
    };
    let handed = source("/opt/orch-check/node");
    assert!(
        handed.starts_with(data.path()),
        "the daemon is handed the node's program where it sees it: {}",
        handed.display()
    );
    assert_eq!(
        std::fs::read(&handed).unwrap(),
        std::fs::read(&runner).unwrap(),
        "the same program"
    );
    assert!(
        !units[0]
            .to_string()
            .contains(&*container.path().to_string_lossy()),
        "no path only the node's container has reaches the daemon"
    );
    // The next check finds the program where it was put.
    assert_eq!(
        orch_node::docker::config_of(&settings)
            .unwrap()
            .live_helper_binary,
        Some(handed)
    );
}

#[tokio::test]
async fn an_unavailable_check_image_is_named_without_hiding_a_daemon_failure() {
    use axum::Router;
    use axum::http::{StatusCode, Uri};
    use std::sync::{Arc, Mutex};
    for status in [StatusCode::NOT_FOUND, StatusCode::SERVICE_UNAVAILABLE] {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let requests = Arc::clone(&seen);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().fallback(move |uri: Uri| {
            requests.lock().unwrap().push(uri.path().to_owned());
            async move {
                (
                    status,
                    axum::Json(serde_json::json!({"message": "check daemon unavailable"})),
                )
            }
        });
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let docker = bollard::Docker::connect_with_http(
            &format!("http://{address}"),
            2,
            bollard::API_DEFAULT_VERSION,
        )
        .unwrap();
        let runner = conformance_runner();
        let config = DockerConfig {
            live_helper_binary: Some(runner.clone()),
            conformance_image: Some("checks.example/native:missing".to_owned()),
            ..Default::default()
        };
        let request = NodeToolConformance {
            name: "codex".to_owned(),
            version: "1.0.0".to_owned(),
            programs: vec!["codex".to_owned()],
            case_timeout_secs: 5,
            timeout_secs: 60,
        };
        let failure = orch_node::conformance_unit::run(
            &docker,
            &config,
            &request,
            &runner,
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        let detail = failure.to_string();
        assert!(detail.contains("checks.example/native:missing"), "{detail}");
        if status == StatusCode::SERVICE_UNAVAILABLE {
            assert!(detail.contains("check daemon unavailable"), "{detail}");
            assert!(!detail.contains("need the preloaded image"), "{detail}");
        }
        assert!(
            seen.lock()
                .unwrap()
                .iter()
                .all(|path| path.contains("/images/"))
        );
        task.abort();
    }
}

//! The node's protected registry file authenticates a real Docker pull, while a wrong login leaks no credential.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    response::Response,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use orch_config::server::NodeDockerRegistry;
use orch_driver_docker::{DockerDriver, progress::Collected};
use orch_node_core::settings::NodeSettings;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Registry {
    login: String,
    config: Vec<u8>,
    config_digest: String,
    manifest: Vec<u8>,
    manifest_digest: String,
    accepted: AtomicUsize,
    refused: AtomicUsize,
}

async fn registry(State(state): State<Arc<Registry>>, request: Request) -> Response {
    if request
        .headers()
        .get("authorization")
        .and_then(|header| header.to_str().ok())
        != Some(state.login.as_str())
    {
        state.refused.fetch_add(1, Ordering::SeqCst);
        return Response::builder().status(StatusCode::UNAUTHORIZED)
            .header("www-authenticate","Basic realm=\"Orch password registry test\"")
            .header("docker-distribution-api-version","registry/2.0")
            .header("content-type","application/json")
            .body(Body::from(json!({"errors":[{"code":"UNAUTHORIZED","message":"wrong-registry-test-password was refused"}]}).to_string())).unwrap();
    }
    state.accepted.fetch_add(1, Ordering::SeqCst);
    let path = request.uri().path();
    let (content, media, digest) = if path == "/v2/" {
        (b"{}".as_slice(), "application/json", "")
    } else if path.starts_with("/v2/lab/unit/manifests/") {
        (
            &state.manifest[..],
            "application/vnd.docker.distribution.manifest.v2+json",
            state.manifest_digest.as_str(),
        )
    } else if path == format!("/v2/lab/unit/blobs/{}", state.config_digest) {
        (
            &state.config[..],
            "application/vnd.docker.container.image.v1+json",
            state.config_digest.as_str(),
        )
    } else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap();
    };
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", media)
        .header("docker-content-digest", digest)
        .header("docker-distribution-api-version", "registry/2.0")
        .header("content-length", content.len())
        .body(if request.method() == axum::http::Method::HEAD {
            Body::empty()
        } else {
            Body::from(content.to_vec())
        })
        .unwrap()
}

#[tokio::test]
async fn a_local_password_registry_uses_the_configured_private_file_and_never_exposes_its_password()
{
    let socket =
        std::env::var("ORCH_TEST_DOCKER_SOCKET").unwrap_or_else(|_| "/var/run/docker.sock".into());
    let Ok(docker) =
        bollard::Docker::connect_with_socket(&socket, 120, bollard::API_DEFAULT_VERSION)
    else {
        eprintln!("not run: no local Docker socket");
        return;
    };
    if docker.ping().await.is_err() {
        eprintln!("not run: the local Docker daemon is unavailable");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let info = docker.info().await.unwrap();
    let architecture = match info.architecture.as_deref().unwrap_or("x86_64") {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    // A valid zero-layer image is sufficient to prove the daemon pulled through Registry V2;
    // no unit or helper is run and the reference is unique to this test.
    let config=serde_json::to_vec(&json!({"architecture":architecture,"os":"linux","config":{"Labels":{"orch.test":orch_types::new_id().to_string()}},"rootfs":{"type":"layers","diff_ids":[]},"history":[]})).unwrap();
    let config_digest = format!("sha256:{}", hex::encode(Sha256::digest(&config)));
    let manifest=serde_json::to_vec(&json!({"schemaVersion":2,"mediaType":"application/vnd.docker.distribution.manifest.v2+json","config":{"mediaType":"application/vnd.docker.container.image.v1+json","size":config.len(),"digest":config_digest},"layers":[]})).unwrap();
    let state = Arc::new(Registry {
        login: format!("Basic {}", STANDARD.encode("tester:registry-test-password")),
        config,
        config_digest,
        manifest_digest: format!("sha256:{}", hex::encode(Sha256::digest(&manifest))),
        manifest,
        accepted: AtomicUsize::new(0),
        refused: AtomicUsize::new(0),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new().fallback(registry).with_state(state.clone()),
        )
        .into_future(),
    );
    let credentials = dir.path().join("registry.json");
    std::fs::write(
        &credentials,
        r#"{"username":"tester","password":"registry-test-password"}"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let mut settings = NodeSettings::in_dir(dir.path());
    settings.docker.socket = socket;
    settings.config_file = Some(dir.path().join("orch.toml"));
    settings.docker.registries.push(NodeDockerRegistry {
        server: address.to_string(),
        credentials_file: "registry.json".into(),
    });
    let configured = orch_node::docker::config_of(&settings).unwrap();
    assert!(!format!("{configured:?} {settings:?}").contains("registry-test-password"));
    let driver = DockerDriver::connect(configured).await.unwrap();
    let reference = format!("{address}/lab/unit:ok-{}", orch_types::new_id().simple());
    let progress = Collected::default();
    let image = driver
        .ensure_image_reporting(&json!({"image":reference}), &progress)
        .await
        .unwrap();
    assert_eq!(
        image.digest.as_deref(),
        Some(state.manifest_digest.as_str())
    );
    assert!(
        state.accepted.load(Ordering::SeqCst) > 0,
        "the protected file authorized real registry requests"
    );
    assert!(!format!("{:?}", progress.lines()).contains("registry-test-password"));
    std::fs::write(
        &credentials,
        r#"{"username":"tester","password":"wrong-registry-test-password"}"#,
    )
    .unwrap();
    let wrong = DockerDriver::connect(orch_node::docker::config_of(&settings).unwrap())
        .await
        .unwrap();
    let wrong_progress = Collected::default();
    let result = wrong
        .ensure_image_reporting(
            &json!({"image":format!("{address}/lab/unit:wrong-{}",orch_types::new_id().simple())}),
            &wrong_progress,
        )
        .await;
    let failed = result.unwrap_err();
    assert!(state.refused.load(Ordering::SeqCst) > 0);
    assert!(
        !format!("{failed:?} {failed} {:?}", wrong_progress.lines())
            .contains("wrong-registry-test-password")
    );
    docker
        .remove_image(
            &reference,
            Some(
                bollard::query_parameters::RemoveImageOptionsBuilder::default()
                    .force(true)
                    .build(),
            ),
            None,
        )
        .await
        .unwrap();
    server.abort();
}

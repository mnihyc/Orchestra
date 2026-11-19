//! Oversized anonymous JSON is refused through real nginx without corrupting the next request.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![cfg(unix)]

use axum::body::Body;
use bytes::Bytes;
use futures::stream;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use hyper_util::rt::TokioIo;
use orch_api_contract::common::{ErrorCode, MAX_BODY_BYTES, Problem};
use orch_api_core::registry::Registry;
use orch_api_core::testing::TestApp;
use orch_pki::ca::{CertificateAuthority, Holder};
use orch_pki::certificates::ServiceName;
use orch_pki::verification::TlsIdentity;
use orch_testkit::database::TestDb;
use orch_types::SystemClock;
use std::convert::Infallible;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

// Loopback ports of this test alone, outside every shared range of the workspace's tests.
const API_PORT: u16 = 41770;
const EDGE_PORT: u16 = 41771;
const BODY_CHECK_PORT: u16 = 41772;
const PORT_NAMES_PORT: u16 = 41773;
// Smaller frames force the API's body limit to be crossed with data still remaining.
const FRAME_BYTES: usize = 64 * 1024;
// Exercise almost the edge's full allowance, reserving a frame for the JSON wrapper.
const PAYLOAD_FILL_BYTES: usize = MAX_BODY_BYTES - FRAME_BYTES;
// Startup/request deadlines allow scheduling delays on the loaded test machine.
const PATIENCE: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(100);
// The fixed problem envelope fits comfortably here; no request body should be echoed.
const ERROR_REPLY_BYTES: usize = 4096;

struct Nginx(String);

impl Drop for Nginx {
    fn drop(&mut self) {
        let _ = Command::new("docker").args(["rm", "-f", &self.0]).output();
    }
}

async fn client() -> hyper::client::conn::http1::SendRequest<Body> {
    let stream = TcpStream::connect(("127.0.0.1", EDGE_PORT)).await.unwrap();
    let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    sender
}

#[tokio::test(flavor = "multi_thread")]
async fn anonymous_known_and_chunked_oversized_json_reaches_the_client_as_413_and_keeps_the_connection_usable()
 {
    let database = TestDb::fresh().await;
    let world = TestApp::new(
        orch_db::Db::from_pool(database.pool.clone()),
        orch_testkit::clock::manual(),
        Registry::new(),
    );
    let mut registry = Registry::new();
    orch_api_core::routes::auth::mount(&mut registry);
    orch_api_core::routes::meta::mount(&mut registry);
    let router = orch_api_core::server::router(world.app, registry);
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    // The deployment entrypoint creates empty includes when HTTPS is not configured.
    std::fs::write(directory.path().join("frontend-https.conf"), "").unwrap();
    std::fs::write(directory.path().join("ports-https.conf"), "").unwrap();
    let authority = CertificateAuthority::create("Edge body test", 30, &SystemClock).unwrap();
    std::fs::write(directory.path().join("ca.pem"), authority.certificate_pem()).unwrap();
    let (key, certificate) = authority
        .issue_with_new_key(
            &Holder::Service {
                service: ServiceName::Api,
                names: vec!["api".to_owned()],
            },
            30,
            &SystemClock,
        )
        .unwrap();
    let tls = orch_api_core::server::browser_tls_config(
        &TlsIdentity {
            chain: vec![certificate],
            key,
        },
        Arc::new(SystemClock),
    )
    .unwrap();
    let listener = TcpListener::bind(("127.0.0.1", API_PORT)).await.unwrap();
    let cancel = CancellationToken::new();
    let api = tokio::spawn(orch_api_core::server::serve_tls(
        listener,
        tls,
        router,
        cancel.clone(),
    ));
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap();
    let config = std::fs::read_to_string(root.join("deploy/nginx.conf"))
        .unwrap()
        .replace(
            "listen 80 default_server",
            &format!("listen 127.0.0.1:{EDGE_PORT} default_server"),
        )
        .replace("127.0.0.1:8081", &format!("127.0.0.1:{BODY_CHECK_PORT}"))
        .replace(
            "        listen 80;\n",
            &format!("        listen 127.0.0.1:{PORT_NAMES_PORT};\n"),
        )
        .replace("https://api:8443", &format!("https://127.0.0.1:{API_PORT}"))
        .replace("/tmp/frontend-https.conf", "/fixture/frontend-https.conf")
        .replace("/tmp/ports-https.conf", "/fixture/ports-https.conf")
        .replace("/etc/orch/secrets/orch_ca_cert.pem", "/fixture/ca.pem");
    std::fs::write(directory.path().join("nginx.conf"), config).unwrap();
    let compose = std::fs::read_to_string(root.join("deploy/compose.yml")).unwrap();
    let image = compose
        .lines()
        .filter_map(|line| line.trim().strip_prefix("image:"))
        .map(str::trim)
        .find(|image| image.starts_with("nginx:"))
        .unwrap();
    let nginx = Nginx(format!("orchkfixdeploy-body-{}", orch_types::new_id()));
    println!("owned nginx fixture: {}", nginx.0);
    let started = Command::new("docker")
        .args([
            "run",
            "-d",
            "--name",
            &nginx.0,
            "--network",
            "host",
            "--read-only",
            "--tmpfs",
            "/tmp",
            "--tmpfs",
            "/var/cache/nginx",
        ])
        .arg("--volume")
        .arg(format!("{}:/fixture:ro", directory.path().display()))
        .arg("--volume")
        .arg(format!(
            "{}:/etc/nginx/nginx.conf:ro",
            directory.path().join("nginx.conf").display()
        ))
        .arg(image)
        .output()
        .unwrap();
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    tokio::time::timeout(PATIENCE, async {
        loop {
            if TcpStream::connect(("127.0.0.1", EDGE_PORT)).await.is_ok() {
                break;
            }
            tokio::time::sleep(POLL).await;
        }
    })
    .await
    .unwrap_or_else(|error| {
        let logs = Command::new("docker")
            .args(["logs", &nginx.0])
            .output()
            .unwrap();
        panic!(
            "edge did not start: {error}; {}",
            String::from_utf8_lossy(&logs.stderr)
        );
    });
    let marker = "credential-that-must-not-be-echoed";
    let payload = format!(
        "{{\"login\":\"{marker}{}\",\"password\":\"also-private\"}}",
        "x".repeat(PAYLOAD_FILL_BYTES)
    );
    let mut sender = client().await;
    let mut statuses = Vec::new();
    for chunked in [false, true] {
        let body = if chunked {
            let pieces: Vec<_> = payload
                .as_bytes()
                .chunks(FRAME_BYTES)
                .map(Bytes::copy_from_slice)
                .collect();
            Body::from_stream(stream::iter(pieces.into_iter().map(Ok::<_, Infallible>)))
        } else {
            Body::from(payload.clone())
        };
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/login")
            .header("host", "127.0.0.1")
            .header("content-type", "application/json")
            .body(body)
            .unwrap();
        let response = tokio::time::timeout(PATIENCE, sender.send_request(request))
            .await
            .unwrap()
            .unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        println!("chunked={chunked}: {status}, {} response bytes", body.len());
        assert!(body.len() < ERROR_REPLY_BYTES);
        assert!(!String::from_utf8_lossy(&body).contains(marker));
        if status == StatusCode::PAYLOAD_TOO_LARGE {
            let problem: Problem = serde_json::from_slice(&body).unwrap();
            assert_eq!(problem.code, ErrorCode::RequestTooLarge);
        }
        statuses.push(status);
        let next = Request::builder()
            .uri("/api/v1/meta")
            .header("host", "127.0.0.1")
            .body(Body::empty())
            .unwrap();
        let next = tokio::time::timeout(PATIENCE, sender.send_request(next))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(next.status(), StatusCode::OK);
        let next = next.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice::<serde_json::Value>(&next).unwrap();
    }
    cancel.cancel();
    api.await.unwrap().unwrap();
    assert_eq!(statuses, vec![StatusCode::PAYLOAD_TOO_LARGE; 2]);
}

//! A release mirror cannot establish the executable's expected checksum by supplying both sides of the check.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::response::Response;
use serde_json::json;

#[tokio::test]
async fn a_mirror_cannot_introduce_a_build_with_a_checksum_only_it_supplies() {
    let digest = "ab".repeat(32);
    let supplied = digest.clone();
    let app = Router::new().fallback(move |request: Request| {
        let supplied = supplied.clone();
        async move {
            let body = if request.uri().path().starts_with("/api.github.com/") {
                json!([{
                    "tag_name": "rust-v999.0.0",
                    "draft": false,
                    "prerelease": false,
                    "assets": [{
                        "name": "codex-package-x86_64-unknown-linux-musl.tar.gz",
                        "digest": format!("sha256:{supplied}"),
                        "browser_download_url": "https://github.com/openai/codex/releases/download/rust-v999.0.0/codex-package-x86_64-unknown-linux-musl.tar.gz"
                    }]
                }]).to_string()
            } else {
                String::new()
            };
            Response::builder()
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap()
        }
    });
    // This review's own fixture listens only in its assigned local port range.
    let mut listener = None;
    for port in 41600..=41649 {
        if let Ok(bound) =
            tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await
        {
            listener = Some(bound);
            break;
        }
    }
    let listener = listener.expect("one security fixture port is available");
    let mirror = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let (builds, _failures) = orch_api_core::harness_channels::read(
        &client,
        orch_agent_programs::Programs::builtin(),
        Some(&mirror),
        &std::collections::BTreeSet::new(),
    )
    .await;
    server.abort();
    let _ = server.await;
    assert!(
        builds.is_empty(),
        "an untrusted mirror established a new executable and its checksum: {:?}",
        builds
            .iter()
            .map(|build| (&build.version, &build.sha256))
            .collect::<Vec<_>>()
    );
}

//! A TLS connection owns its transport and peer identity while sharing the mounted routes.

use axum::extract::{ConnectInfo, Extension, Request};
use axum::{Router, routing::get};
use orch_api_core::server::{self, Peer};
use orch_pki::ca::{CertificateAuthority, Holder};
use orch_pki::certificates::ServiceName;
use orch_pki::verification::TlsIdentity;
use orch_types::SystemClock;
use std::collections::HashSet;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn five_tls_clients_share_the_routes_and_each_request_keeps_its_own_peer() {
    let authority = CertificateAuthority::create("Connection test", 30, &SystemClock).unwrap();
    let (key, certificate) = authority
        .issue_with_new_key(
            &Holder::Service {
                service: ServiceName::Api,
                names: vec!["api.test".into()],
            },
            10,
            &SystemClock,
        )
        .unwrap();
    let tls = server::browser_tls_config(
        &TlsIdentity {
            chain: vec![certificate],
            key,
        },
        Arc::new(SystemClock),
    )
    .unwrap();
    let ca = reqwest::Certificate::from_pem(authority.certificate_pem().as_bytes()).unwrap();
    let listener = super::listener().await;
    let address = listener.local_addr().unwrap();
    let copies = Arc::new(AtomicUsize::new(0));
    let unused = copies.clone();
    let router =
        Router::new()
            .route_service(
                "/unused",
                tower::service_fn(move |_: Request| {
                    unused.fetch_add(1, Ordering::Relaxed);
                    std::future::ready(Ok::<_, Infallible>(http::StatusCode::NO_CONTENT))
                }),
            )
            .route(
                "/peer",
                get(
                    |Extension(peer): Extension<Peer>,
                     ConnectInfo(remote): ConnectInfo<SocketAddr>| async move {
                        assert_eq!(peer.remote, remote);
                        assert!(peer.service.is_none() && peer.certificate.is_none());
                        remote.to_string()
                    },
                ),
            );
    let initial = Arc::strong_count(&copies);
    let stop = CancellationToken::new();
    let serving = tokio::spawn(server::serve_tls(listener, tls, router, stop.clone()));
    let url = format!("https://api.test:{}/peer", address.port());
    // Reconnect the same five-client workload. Each client keeps its connection for a second read.
    for _ in 0..3 {
        let mut clients = Vec::new();
        let mut peers = HashSet::new();
        for _ in 0..5 {
            let client = reqwest::Client::builder()
                .no_proxy()
                .resolve("api.test", address)
                .tls_certs_merge([ca.clone()])
                .build()
                .unwrap();
            let peer = client
                .get(&url)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .text()
                .await
                .unwrap();
            assert!(peers.insert(peer.clone()));
            clients.push((client, peer));
        }
        for (client, peer) in &clients {
            assert_eq!(
                client
                    .get(&url)
                    .send()
                    .await
                    .unwrap()
                    .error_for_status()
                    .unwrap()
                    .text()
                    .await
                    .unwrap(),
                *peer
            );
        }
        assert_eq!(
            Arc::strong_count(&copies),
            initial,
            "an idle connection kept a copy of an unrelated route"
        );
    }
    assert_eq!(copies.load(Ordering::Relaxed), 0);
    stop.cancel();
    serving.await.unwrap().unwrap();
}

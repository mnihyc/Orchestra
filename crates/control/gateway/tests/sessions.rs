//! Sessions: one for each machine, replaced by a newer, through either port; and the requests beside them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, Method, Uri};
use axum::routing::any;
use common::{PATIENCE, World, eventually, request};
use orch_api_core::state::App;
use orch_link::machine::LinkStatus;
use orch_link::reconnect::Dialer;
use orch_link::session::{ClientIdentity, connect};
use orch_link::websocket::TlsDialer;
use orch_link_contract::enrollment::Trust;
use orch_link_contract::session::{Arch, Os, Platform, Role};
use orch_pki::certificates::ServiceName;
use orch_pki::verification::{ServerTrust, pinned_client_config, public_client_config, root_store};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn a_second_session_of_the_same_machine_replaces_the_first() {
    let mut world = World::start().await;
    let enrolled = world.enroll("alice-laptop").await;
    let first = world.start_machine(&enrolled, enrolled.endpoints(Trust::Pinned));
    tokio::time::timeout(PATIENCE, first.link.connected())
        .await
        .unwrap();
    let id = first.id;
    eventually("the API to know the first session", || {
        let link = world.api.link.clone();
        async move { link.session_of(id).is_some() }
    })
    .await;
    let first_session = world.api.link.session_of(id).unwrap();

    // The same machine connects again, from another process.
    let second = world.start_machine(&enrolled, enrolled.endpoints(Trust::Pinned));
    tokio::time::timeout(PATIENCE, second.link.connected())
        .await
        .unwrap();

    // The first is told that it is replaced.
    let mut status = first.link.status();
    let told = tokio::time::timeout(PATIENCE, async {
        loop {
            if let LinkStatus::Waiting { why, .. } = &*status.borrow_and_update()
                && why.contains("Replaced")
            {
                return why.clone();
            }
            status.changed().await.unwrap();
        }
    })
    .await
    .expect("the first session to be told that it was replaced");
    assert!(told.contains("Replaced"), "{told}");
    // It must not win the place back from the second, which is the one that stays.
    first.link.shutdown().await;

    let LinkStatus::Connected { session_id, .. } = second.link.status().borrow().clone() else {
        panic!("the second session is up");
    };
    eventually("the API to hold the second session", || {
        let link = world.api.link.clone();
        async move { link.session_of(id) == Some(session_id) }
    })
    .await;
    assert_ne!(session_id, first_session);
    assert_eq!(
        world.gateway.gateway.sessions_held(),
        1,
        "one session for one machine"
    );
    world.stop().await;
}

#[tokio::test]
async fn a_machine_behind_the_public_edge_proves_itself_by_signing_what_the_gateway_sent() {
    let mut world = World::start().await;
    let enrolled = world.enroll("alice-laptop").await;
    let machine = world.start_machine(&enrolled, enrolled.endpoints(Trust::Public));
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .unwrap();
    let id = machine.id;
    eventually("the API to see the machine", || {
        let link = world.api.link.clone();
        async move { link.is_online(id) }
    })
    .await;
    let LinkStatus::Connected { endpoint, .. } = machine.link.status().borrow().clone() else {
        panic!("connected");
    };
    assert_eq!(endpoint, enrolled.endpoints(Trust::Public)[0].url);
    world.stop().await;
}

/// What the stand-in for the store of content saw of a request.
async fn echo(
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> axum::Json<serde_json::Value> {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    axum::Json(serde_json::json!({
        "method": method.as_str(),
        "path": uri.path(),
        "query": uri.query(),
        "body_bytes": body.len(),
        "machine": header("x-orch-machine"),
        "fingerprint": header("x-orch-machine-fingerprint"),
        "client_ip": header("x-orch-client-ip"),
        "command": header("x-orch-command"),
        "authorization": header("authorization"),
    }))
}

fn echo_router() -> Router<App> {
    Router::new().route("/internal/link/blobs/{*rest}", any(echo))
}

#[tokio::test]
async fn content_requests_reach_the_system_with_who_asks_and_nothing_a_machine_claims_about_itself()
{
    let mut world = World::start_with(echo_router()).await;
    let enrolled = world.enroll("alice-laptop").await;
    let hash = "a".repeat(64);
    let path = format!("/link/v1/blobs/{hash}?offset=0&size=5");
    let spoof = [
        ("x-orch-machine", "00000000-0000-0000-0000-000000000001"),
        ("x-orch-client-ip", "203.0.113.9"),
        ("x-orch-machine-fingerprint", "ff"),
        ("x-orch-command", "0199c1a0-0000-7000-8000-00000000000a"),
    ];

    // On the pinned port, who asks is the certificate of the connection.
    let tls = world.pinned_config(&enrolled);
    let (status, body) = common::request(
        world.gateway.pinned,
        Arc::clone(&tls),
        "PUT",
        &path,
        Some("hello"),
        &spoof,
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let seen: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        seen["machine"],
        enrolled.identity.machine_id.to_string(),
        "not the machine it claimed"
    );
    assert_eq!(
        seen["fingerprint"],
        enrolled.identity.certificate.fingerprint()
    );
    assert_eq!(seen["client_ip"], "127.0.0.1", "not the address it claimed");
    assert_eq!(
        seen["command"], "0199c1a0-0000-7000-8000-00000000000a",
        "what is its own to say is passed on"
    );
    assert_eq!(seen["path"], format!("/internal/link/blobs/{hash}"));
    assert_eq!(seen["query"], "offset=0&size=5");
    assert_eq!(seen["body_bytes"], 5);
    assert_eq!(seen["method"], "PUT");

    // Without a certificate, the pinned port knows nobody.
    let anonymous = pinned_client_config(
        ServerTrust::Authority(Box::new(world.secrets.ca_certificate())),
        Some(ServiceName::Gateway),
        None,
        Arc::clone(&world.clock),
    )
    .unwrap();
    let (status, _) = request(world.gateway.pinned, anonymous, "GET", &path, None, &[]).await;
    assert_eq!(status, 401);

    // Behind the edge, who asks is the session that gave the token.
    let public = public_client_config(
        root_store(&[world.secrets.ca_certificate()]).unwrap(),
        Arc::clone(&world.clock),
    )
    .unwrap();
    let endpoint = enrolled.endpoints(Trust::Public).remove(0);
    let connection = TlsDialer::new(None, Some(Arc::clone(&public)))
        .dial(&endpoint)
        .await
        .unwrap();
    let established = connect(
        connection,
        &ClientIdentity {
            hosted_endpoints: None,
            endpoints: None,
            machine_id: enrolled.identity.machine_id,
            connector_version: "0.1.0".to_owned(),
            platform: Platform {
                os: Os::Linux,
                arch: Arch::X86_64,
            },
            roles: vec![Role::Agent],
            proof: Some((
                enrolled.identity.key.duplicate().unwrap(),
                enrolled.identity.certificate.clone(),
            )),
        },
        world.clock.as_ref(),
    )
    .await
    .unwrap();
    let token = established.ready.session_token.clone();
    let authorization = format!("OrchLink {token}");
    let (status, body) = request(
        world.gateway.edge,
        Arc::clone(&public),
        "GET",
        &path,
        None,
        &[
            ("authorization", &authorization),
            ("x-orch-machine", "00000000-0000-0000-0000-000000000001"),
        ],
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let seen: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(seen["machine"], enrolled.identity.machine_id.to_string());
    assert_eq!(
        seen["authorization"],
        serde_json::Value::Null,
        "the token stays at the gateway"
    );

    // A token nobody was given, and none at all, are not good.
    let (status, _) = request(
        world.gateway.edge,
        Arc::clone(&public),
        "GET",
        &path,
        None,
        &[("authorization", "OrchLink nonsense")],
    )
    .await;
    assert_eq!(status, 401);
    let (status, _) = request(
        world.gateway.edge,
        Arc::clone(&public),
        "GET",
        &path,
        None,
        &[],
    )
    .await;
    assert_eq!(status, 401);
    // And nothing else of the system is reached by this way.
    let (status, _) = request(
        world.gateway.edge,
        public,
        "GET",
        "/internal/backplane",
        None,
        &[],
    )
    .await;
    assert_eq!(status, 404);
    drop(established);
    tokio::time::sleep(Duration::from_millis(100)).await;
    world.stop().await;
}

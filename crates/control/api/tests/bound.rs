//! What is bound of the sign-in part of the contract, and the health check beside it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use axum::body::Body;
use http::{Request, StatusCode};
use orch_api_core::registry::Registry;
use std::collections::HashSet;
use support::World;
use tower::ServiceExt;

#[test]
fn every_request_of_signing_in_is_bound_single_sign_on_and_its_test_included() {
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let bound: HashSet<&str> = registry.bound().iter().map(|route| route.name).collect();
    for route in orch_api_contract::auth::ROUTES {
        assert!(bound.contains(route.name), "{}", route.name);
    }
    // The contract's requests of signing in, the session's As admin switch and its openings among them.
    assert_eq!(orch_api_contract::auth::ROUTES.len(), 42);
    assert!(bound.contains("AdminIdentityProviderTest"));
}

async fn get(router: &axum::Router, path: &str) -> StatusCode {
    router
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn readiness_needs_the_database_and_a_worker_beat_no_older_than_its_limit() {
    let world = World::new().await;
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = orch_api_core::server::router(world.app.app.clone(), registry);
    for path in ["/health", "/api/v1/health"] {
        assert_eq!(
            get(&router, path).await,
            StatusCode::SERVICE_UNAVAILABLE,
            "no worker has reported"
        );
    }
    let seen = world.now();
    orch_db::heartbeat::beat(
        &mut *world.conn().await,
        "worker",
        orch_types::new_id(),
        "test",
        seen,
        seen,
    )
    .await
    .unwrap();
    assert_eq!(get(&router, "/health").await, StatusCode::OK);
    assert_eq!(get(&router, "/api/v1/health").await, StatusCode::OK);
    world.advance(orch_db::heartbeat::WORKER_HEARTBEAT_MAX_AGE);
    assert_eq!(
        get(&router, "/health").await,
        StatusCode::OK,
        "the exact boundary is current"
    );
    world.advance(chrono::Duration::milliseconds(1));
    for path in ["/health", "/api/v1/health"] {
        assert_eq!(
            get(&router, path).await,
            StatusCode::SERVICE_UNAVAILABLE,
            "the worker stopped reporting"
        );
    }
    let now = world.now();
    orch_db::heartbeat::beat(
        &mut *world.conn().await,
        "worker",
        orch_types::new_id(),
        "test",
        now,
        now,
    )
    .await
    .unwrap();
    assert_eq!(
        get(&router, "/health").await,
        StatusCode::OK,
        "a current worker restores readiness"
    );
    world.db.pool.close().await;
    assert_eq!(
        get(&router, "/health").await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        get(&router, "/api/v1/health").await,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

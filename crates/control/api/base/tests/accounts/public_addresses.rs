//! Publication waits for the same installation at the candidate browser origin.

use super::*;
use orch_api_contract::public_addresses::PublicAddresses;
use orch_api_core::public_addresses::{self as effective, PrepareOptions};
use std::time::Duration as Wait;

struct Edge {
    url: String,
    serving: tokio::task::JoinHandle<()>,
}

impl Drop for Edge {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

async fn edge(world: &World) -> Edge {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let mut registry = orch_api_core::registry::Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = axum::Router::new()
        .nest("/api/v1", registry.into_router())
        .with_state(world.app.app.clone());
    let serving = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Edge { url, serving }
}

fn external() -> PrepareOptions {
    PrepareOptions {
        managed_frontend: None,
        timeout: Wait::from_secs(10),
        poll: Wait::from_millis(25),
    }
}

#[tokio::test]
async fn a_candidate_must_prove_this_installation_before_its_prepared_revision_can_be_published() {
    let world = World::new().await;
    let entrance = edge(&world).await;
    let before = effective::current(&world.app.app).await.unwrap();
    let wire = serde_json::to_value(&before).unwrap();
    assert_eq!(
        serde_json::from_value::<orch_api_contract::public_addresses::PublicAddressSnapshot>(wire)
            .unwrap(),
        before,
        "a snapshot's flattened address fields, revision and seed state are one readable reply"
    );
    let candidate = PublicAddresses {
        public_url: entrance.url.clone(),
        alternate_urls: Vec::new(),
    };
    let mut prepared = effective::prepare(
        &world.app.app,
        candidate.clone(),
        before.revision,
        external(),
    )
    .await
    .unwrap();
    assert_eq!(
        effective::current(&world.app.app).await.unwrap(),
        before,
        "a successful preparation is still unpublished"
    );
    let mut tx = world.app.app.db.begin().await.unwrap();
    let after = prepared.publish(&mut tx, None, world.now()).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(after.addresses, candidate);
    assert_eq!(effective::current(&world.app.app).await.unwrap(), after);
}

#[tokio::test]
async fn another_installation_at_a_candidate_origin_cannot_replace_the_current_address() {
    let world = World::new().await;
    let other = World::new().await;
    let entrance = edge(&other).await;
    let before = effective::current(&world.app.app).await.unwrap();
    let candidate = PublicAddresses {
        public_url: entrance.url.clone(),
        alternate_urls: Vec::new(),
    };
    let refused = effective::prepare(&world.app.app, candidate, before.revision, external())
        .await
        .err()
        .expect("a different installation is refused");
    assert_eq!(refused.code(), ErrorCode::PublicAddressMismatch);
    assert_eq!(effective::current(&world.app.app).await.unwrap(), before);
}

#[tokio::test]
async fn a_reachable_primary_does_not_publish_an_unproved_alternate_or_a_stale_draft() {
    let world = World::new().await;
    let other = World::new().await;
    let primary = edge(&world).await;
    let alternate = edge(&other).await;
    let before = effective::current(&world.app.app).await.unwrap();
    let candidate = PublicAddresses {
        public_url: primary.url.clone(),
        alternate_urls: vec![alternate.url.clone()],
    };
    assert!(
        effective::prepare(&world.app.app, candidate, before.revision, external())
            .await
            .is_err()
    );
    assert_eq!(effective::current(&world.app.app).await.unwrap(), before);
    let candidate = PublicAddresses {
        public_url: "http://127.0.0.1:1".into(),
        alternate_urls: Vec::new(),
    };
    let refused = effective::prepare(&world.app.app, candidate, before.revision - 1, external())
        .await
        .err()
        .expect("stale before any network attempt");
    assert_eq!(refused.code(), ErrorCode::ConflictStaleRevision);
}

#[cfg(unix)]
#[path = "public_addresses_native.rs"]
mod native;

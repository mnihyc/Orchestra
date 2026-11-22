//! Public-address management keeps its native system governor through preparation and publication.
use super::*;
use orch_api_contract::public_addresses::*;

struct Edge {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Edge {
    fn drop(&mut self) {
        self.task.abort();
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
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Edge { url, task }
}
async fn address(world: &World, root: &Who) -> PublicAddressSnapshot {
    world
        .app
        .call(PublicAddressGet, &[])
        .token(&root.token)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn a_manage_only_system_key_gets_an_intrinsic_address_receipt_and_cannot_read_the_system_projection()
 {
    let world = World::new().await;
    let root = admin(&world, "root").await;
    let before = address(&world, &root).await;
    let server = edge(&world).await;
    let key = scoped_key(&world, &root, "PublicAddressUpdate", &["manage"]).await;
    let body = PublicAddressUpdateBody {
        expected_revision: before.revision,
        public_url: server.url.clone(),
        alternate_urls: Vec::new(),
    };
    let reply = world
        .app
        .call(PublicAddressUpdate, &[])
        .token(&key)
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.raw));
    let receipt: Json = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["public_url"], server.url);
    assert_eq!(
        receipt.as_object().unwrap().len(),
        5,
        "only the caller's addresses, accepted revision and example state accompany the receipt marker"
    );
    assert!(!receipt.to_string().contains("seed"));
    assert_eq!(
        world
            .app
            .call(PublicAddressGet, &[])
            .token(&key)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let after = address(&world, &root).await;
    assert_eq!(after.addresses.public_url, body.public_url);
    assert!(!after.from_file_seed);
    let meta = world
        .app
        .call(orch_api_contract::auth::MetaGet, &[])
        .send()
        .await
        .ok();
    assert_eq!(meta.public_address_revision, after.revision);
    assert_eq!(meta.public_url, after.addresses.public_url);
    let stale = world
        .app
        .call(PublicAddressUpdate, &[])
        .token(&key)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert!(!stale.extra.contains_key("current"));
}

#[tokio::test]
async fn an_org_owner_or_a_read_only_system_key_cannot_prepare_or_publish_an_address() {
    let world = World::new().await;
    let root = admin(&world, "root").await;
    let owner = person(&world, "owner").await;
    let before = address(&world, &root).await;
    let read = scoped_key(&world, &root, "PublicAddressGet", &["read"]).await;
    let body = PublicAddressUpdateBody {
        expected_revision: before.revision,
        public_url: "http://127.0.0.1:1".into(),
        alternate_urls: Vec::new(),
    };
    for token in [&owner.token, &read] {
        assert_eq!(
            world
                .app
                .call(PublicAddressUpdate, &[])
                .token(token)
                .body(&body)
                .send()
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied
        );
    }
    assert_eq!(address(&world, &root).await, before);
}

#[tokio::test]
async fn an_unproved_origin_leaves_the_applied_revision_and_both_address_fields_unchanged() {
    let world = World::new().await;
    let other = World::new().await;
    let root = admin(&world, "root").await;
    let before = address(&world, &root).await;
    let wrong = edge(&other).await;
    let body = PublicAddressUpdateBody {
        expected_revision: before.revision,
        public_url: wrong.url.clone(),
        alternate_urls: vec!["https://unproved.invalid".into()],
    };
    assert_eq!(
        world
            .app
            .call(PublicAddressUpdate, &[])
            .token(&root.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::PublicAddressMismatch
    );
    assert_eq!(address(&world, &root).await, before);
}

#[tokio::test]
async fn management_says_when_the_shipped_example_is_still_the_applied_primary() {
    let world = World::with(orch_config::ServerConfig::default()).await;
    let root = admin(&world, "root").await;
    let applied = address(&world, &root).await;
    assert!(applied.using_example);
    assert_eq!(applied.addresses.public_url, SHIPPED_PUBLIC_ORIGIN);
}

#[tokio::test]
async fn losing_system_administration_during_a_held_origin_proof_prevents_publication() {
    use std::sync::Arc;
    let world = World::new().await;
    let root = admin(&world, "root").await;
    let other = admin(&world, "other").await;
    let before = address(&world, &root).await;
    let reached = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let mut registry = orch_api_core::registry::Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = axum::Router::new()
        .nest("/api/v1", registry.into_router())
        .with_state(world.app.app.clone())
        .layer(axum::middleware::from_fn({
            let reached = reached.clone();
            let release = release.clone();
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let reached = reached.clone();
                let release = release.clone();
                async move {
                    reached.notify_one();
                    release.notified().await;
                    next.run(request).await
                }
            }
        }));
    let _server = Edge {
        url: url.clone(),
        task: tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }),
    };
    let expected_revision = before.revision;
    let app = world.app.clone();
    let token = root.token.clone();
    let pending = tokio::spawn(async move {
        app.call(PublicAddressUpdate, &[])
            .token(&token)
            .body(&PublicAddressUpdateBody {
                expected_revision,
                public_url: url,
                alternate_urls: Vec::new(),
            })
            .send()
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), reached.notified())
        .await
        .unwrap();
    sqlx::query("UPDATE users SET is_system_admin=false WHERE id=$1")
        .bind(root.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    release.notify_one();
    assert_eq!(
        pending.await.unwrap().problem().code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(address(&world, &other).await, before);
}

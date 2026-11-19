//! Real private upload custody, scoped release and the actual blob-row wait.

use super::support::World;
use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::threads::{UploadBlobDelete, UploadBlobDeleteQuery};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::{identity, uploads};
use orch_types::Json;

fn rule(operation: &str, kind: &str, id: Id, action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":["upload"],
        "allow":[{"operation":operation,"actions":[action]}]})
}

async fn key(world: &World, actor: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: actor,
            name: "Native upload custody fixture",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    secret
}

async fn whole(world: &World, actor: &Who, bytes: &[u8]) -> Hash {
    world
        .app
        .call(UploadCreate, &[])
        .token(&actor.token)
        .header("content-type", "image/png")
        .header(FILENAME_HEADER, "private-upload.png")
        .bytes(bytes)
        .send()
        .await
        .ok()
        .blob
}

async fn owns(world: &World, user: Id, hash: &Hash) -> bool {
    uploads::owns(&mut *world.conn().await, user, hash)
        .await
        .unwrap()
}

#[tokio::test]
async fn upload_release_binds_the_actual_custody_account_and_keeps_other_uploaders() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let other = places::person(&world, "other").await;
    let hash = whole(&world, &owner, b"native shared content").await;
    assert_eq!(whole(&world, &other, b"native shared content").await, hash);
    for refused in [
        rule("UploadBlobDelete", "account", other.id(), "write"),
        rule("UploadBlobDelete", "account", owner.id(), "read"),
        rule("UploadSessionCreate", "account", owner.id(), "write"),
    ] {
        let secret = key(&world, owner.id(), vec![refused]).await;
        let reply = world
            .app
            .call(UploadBlobDelete, &[&hash])
            .token(&secret)
            .query(&UploadBlobDeleteQuery::default())
            .send_bytes()
            .await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert!(owns(&world, owner.id(), &hash).await);
        assert!(owns(&world, other.id(), &hash).await);
    }
    let writer = key(
        &world,
        owner.id(),
        vec![rule("UploadBlobDelete", "account", owner.id(), "write")],
    )
    .await;
    for _ in 0..2 {
        let reply = world
            .app
            .call(UploadBlobDelete, &[&hash])
            .token(&writer)
            .query(&UploadBlobDeleteQuery::default())
            .send_bytes()
            .await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
        assert!(reply.raw.is_empty());
        assert!(!owns(&world, owner.id(), &hash).await);
        assert!(owns(&world, other.id(), &hash).await);
    }
    let missing = Hash::of(b"no custody reference");
    assert!(!owns(&world, owner.id(), &missing).await);
    let reply = world
        .app
        .call(UploadBlobDelete, &[&missing])
        .token(&writer)
        .query(&UploadBlobDeleteQuery::default())
        .send_bytes()
        .await;
    assert_eq!(
        reply.status,
        StatusCode::NO_CONTENT,
        "the private release remains idempotent"
    );
}

#[tokio::test]
async fn upload_release_uses_its_typed_private_reservation_without_claiming_thread_authority() {
    let site = Site::new().await;
    let first = attach_raw(
        &site,
        &site.ada,
        site.thread,
        "kept.txt",
        b"same private bytes",
    )
    .await
    .ok();
    let second = attach_raw(
        &site,
        &site.ada,
        site.thread,
        "kept.txt",
        b"same private bytes",
    )
    .await
    .ok();
    assert_eq!(first.blob, second.blob);
    assert_ne!(first.path, second.path);
    let pins = || async {
        storage::blob(&mut *site.world.conn().await, &first.blob)
            .await
            .unwrap()
            .unwrap()
            .pin_count
    };
    let before = pins().await;
    let thread_only = key(
        &site.world,
        site.ada.id(),
        vec![rule("UploadBlobDelete", "thread", site.thread, "write")],
    )
    .await;
    let query = UploadBlobDeleteQuery {
        thread: Some(site.thread),
        path: Some(second.path),
    };
    let refused = site
        .app()
        .call(UploadBlobDelete, &[&first.blob])
        .token(&thread_only)
        .query(&query)
        .send_bytes()
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(pins().await, before);
    let writer = key(
        &site.world,
        site.ada.id(),
        vec![rule("UploadBlobDelete", "account", site.ada.id(), "write")],
    )
    .await;
    for _ in 0..2 {
        let reply = site
            .app()
            .call(UploadBlobDelete, &[&first.blob])
            .token(&writer)
            .query(&query)
            .send_bytes()
            .await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
        assert!(reply.raw.is_empty());
        assert_eq!(pins().await, before - 1);
    }
    let remains: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_attachment_uploads WHERE created_by=$1 AND blob=$2 AND thread_id=$3 AND path=$4)")
        .bind(site.ada.id()).bind(first.blob.as_bytes().as_slice()).bind(site.thread).bind(&first.path)
        .fetch_one(&site.world.db.pool).await.unwrap();
    assert!(remains);
    let malformed = site
        .app()
        .call(UploadBlobDelete, &[&first.blob])
        .token(&writer)
        .query(&UploadBlobDeleteQuery {
            thread: Some(site.thread),
            path: None,
        })
        .send_bytes()
        .await;
    assert_eq!(malformed.status, StatusCode::BAD_REQUEST);
    assert_eq!(pins().await, before - 1);
}

#[tokio::test]
async fn upload_release_rechecks_the_key_after_the_actual_blob_row_wait_before_deleting_custody() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let hash = whole(&world, &owner, b"retained at native wait").await;
    let writer = key(
        &world,
        owner.id(),
        vec![rule("UploadBlobDelete", "account", owner.id(), "write")],
    )
    .await;
    let mut blocker = world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT hash FROM blobs WHERE hash=$1 FOR UPDATE")
        .bind(hash.as_bytes().as_slice())
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let request = tokio::spawn(async move {
        let call = app
            .call(UploadBlobDelete, &[&hash])
            .token(&writer)
            .query(&UploadBlobDeleteQuery::default());
        call.send_bytes().await
    });
    // Match the native wait fixture watchdog; the real blocking SQL is the premise.
    let wait = std::time::Duration::from_secs(30);
    tokio::time::timeout(wait, async {
        loop {
            assert!(!request.is_finished(), "the native blob wait was not reached");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%SELECT hash FROM blobs%')")
                .bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the real blob row must serialize the unstarted release");
    world.advance(Duration::hours(2));
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert!(
        owns(&world, owner.id(), &hash).await,
        "custody must remain unchanged before its first effect"
    );
}

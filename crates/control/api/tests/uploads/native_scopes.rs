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

async fn unfinished(world: &World, actor: &Who) -> Id {
    world
        .app
        .call(UploadSessionCreate, &[])
        .token(&actor.token)
        .body(&UploadSessionCreateBody {
            name: "private-session-name.txt".into(),
            size: 20,
            media_type: Some("text/plain".into()),
        })
        .send()
        .await
        .ok()
        .upload_id
}

async fn piece(world: &World, actor: &Who, upload: Id) {
    let reply = world
        .app
        .call(UploadSessionPut, &[&upload])
        .token(&actor.token)
        .query(&UploadSessionPutQuery { offset: 0 })
        .bytes(b"partial")
        .send()
        .await
        .ok();
    assert_eq!(reply.received, 7);
}

async fn cancelled(world: &World, upload: Id) -> bool {
    sqlx::query_scalar("SELECT cancelled_at IS NOT NULL FROM upload_sessions WHERE id=$1")
        .bind(upload)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn upload_session_cancel_uses_exact_native_identity_and_preserves_write_only_cleanup() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let first = unfinished(&world, &owner).await;
    let second = unfinished(&world, &owner).await;
    piece(&world, &owner, first).await;
    piece(&world, &owner, second).await;
    let part = world.app.app.blobs.part_of_session(first);
    let other = world.app.app.blobs.part_of_session(second);
    assert!(part.exists() && other.exists());
    for refused in [
        rule("UploadSessionDelete", "upload", second, "write"),
        rule("UploadSessionDelete", "upload", first, "read"),
        rule("UploadSessionComplete", "upload", first, "write"),
        rule("UploadSessionDelete", "account", owner.id(), "write"),
    ] {
        let secret = key(&world, owner.id(), vec![refused]).await;
        let reply = world
            .app
            .call(UploadSessionDelete, &[&first])
            .token(&secret)
            .send_bytes()
            .await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert!(!String::from_utf8_lossy(&reply.raw).contains("private-session-name"));
        assert!(!cancelled(&world, first).await);
        assert!(part.exists() && other.exists());
    }
    let secret = key(
        &world,
        owner.id(),
        vec![rule("UploadSessionDelete", "upload", first, "write")],
    )
    .await;
    let reply = world
        .app
        .call(UploadSessionDelete, &[&first])
        .token(&secret)
        .send_bytes()
        .await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert!(
        reply.raw.is_empty(),
        "the existing body-free receipt needs no Read"
    );
    assert!(cancelled(&world, first).await);
    assert!(!part.exists());
    assert!(!cancelled(&world, second).await);
    assert!(
        other.exists(),
        "only the selected session's temporary bytes are discarded"
    );
    let again = world
        .app
        .call(UploadSessionDelete, &[&first])
        .token(&secret)
        .send_bytes()
        .await;
    assert_eq!(
        again.status,
        StatusCode::NOT_FOUND,
        "native closed-session semantics remain"
    );
}

#[tokio::test]
async fn upload_session_scope_does_not_lend_native_owner_authority_or_session_lifetime() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let other = places::person(&world, "other").await;
    let upload = unfinished(&world, &owner).await;
    piece(&world, &owner, upload).await;
    let secret = key(
        &world,
        other.id(),
        vec![rule("UploadSessionDelete", "upload", upload, "write")],
    )
    .await;
    let reply = world
        .app
        .call(UploadSessionDelete, &[&upload])
        .token(&secret)
        .send_bytes()
        .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert!(!cancelled(&world, upload).await);
    sqlx::query("UPDATE upload_sessions SET expires_at=$2 WHERE id=$1")
        .bind(upload)
        .bind(world.now() - Duration::seconds(1))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let secret = key(
        &world,
        owner.id(),
        vec![rule("UploadSessionDelete", "upload", upload, "write")],
    )
    .await;
    let reply = world
        .app
        .call(UploadSessionDelete, &[&upload])
        .token(&secret)
        .send_bytes()
        .await;
    assert_eq!(reply.status, StatusCode::GONE);
    assert!(!cancelled(&world, upload).await);
    assert!(world.app.app.blobs.part_of_session(upload).exists());
}

#[tokio::test]
async fn upload_session_cancel_rechecks_the_actual_key_after_its_native_row_wait() {
    for expire in [false, true] {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let upload = unfinished(&world, &owner).await;
        piece(&world, &owner, upload).await;
        let secret = key(
            &world,
            owner.id(),
            vec![rule("UploadSessionDelete", "upload", upload, "write")],
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM upload_sessions WHERE id=$1 FOR UPDATE")
            .bind(upload)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let request = tokio::spawn(async move {
            let call = app.call(UploadSessionDelete, &[&upload]).token(&secret);
            call.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait, async {
            loop {
                assert!(!request.is_finished(), "the native session wait must be reached");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM upload_sessions%FOR UPDATE%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual native upload row serializes cancellation");
        if expire {
            world.advance(Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        assert_eq!(
            reply.status,
            if expire {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::NO_CONTENT
            }
        );
        assert_eq!(cancelled(&world, upload).await, !expire);
        assert_eq!(world.app.app.blobs.part_of_session(upload).exists(), expire);
    }
}

const COMPLETE_BYTES: &[u8] = b"0123456789abcdefghij";

async fn filled(world: &World, actor: &Who) -> Id {
    let upload = unfinished(world, actor).await;
    let stored = world
        .app
        .call(UploadSessionPut, &[&upload])
        .token(&actor.token)
        .query(&UploadSessionPutQuery { offset: 0 })
        .bytes(COMPLETE_BYTES)
        .send()
        .await
        .ok();
    assert_eq!(stored.received, 20);
    upload
}

#[tokio::test]
async fn upload_session_completion_is_its_own_write_and_returns_the_native_new_blob_receipt() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let upload = filled(&world, &owner).await;
    let other = unfinished(&world, &owner).await;
    for refused in [
        rule("UploadSessionComplete", "upload", other, "write"),
        rule("UploadSessionComplete", "upload", upload, "read"),
        rule("UploadSessionDelete", "upload", upload, "write"),
    ] {
        let secret = key(&world, owner.id(), vec![refused]).await;
        let reply = world
            .app
            .call(UploadSessionComplete, &[&upload])
            .token(&secret)
            .send_bytes()
            .await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert!(!owns(&world, owner.id(), &Hash::of(COMPLETE_BYTES)).await);
    }
    let secret = key(
        &world,
        owner.id(),
        vec![rule("UploadSessionComplete", "upload", upload, "write")],
    )
    .await;
    let reply = world
        .app
        .call(UploadSessionComplete, &[&upload])
        .token(&secret)
        .send()
        .await
        .ok();
    assert_eq!(reply.blob, Hash::of(COMPLETE_BYTES));
    assert_eq!(reply.size, 20);
    assert_eq!(reply.name, "private-session-name.txt");
    assert_eq!(reply.media_type, "text/plain");
    assert!(owns(&world, owner.id(), &reply.blob).await);
    assert_eq!(
        world
            .app
            .app
            .blobs
            .read_whole(&reply.blob, 20)
            .await
            .unwrap()
            .as_ref(),
        COMPLETE_BYTES
    );
    assert!(!world.app.app.blobs.part_of_session(upload).exists());
    let again = world
        .app
        .call(UploadSessionComplete, &[&upload])
        .token(&secret)
        .send_bytes()
        .await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn upload_session_completion_keeps_the_native_empty_file_and_owner_rules() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let other = places::person(&world, "other").await;
    let upload = world
        .app
        .call(UploadSessionCreate, &[])
        .token(&owner.token)
        .body(&UploadSessionCreateBody {
            size: 0,
            name: "empty.txt".into(),
            media_type: None,
        })
        .send()
        .await
        .ok()
        .upload_id;
    let foreign = key(
        &world,
        other.id(),
        vec![rule("UploadSessionComplete", "upload", upload, "write")],
    )
    .await;
    let refused = world
        .app
        .call(UploadSessionComplete, &[&upload])
        .token(&foreign)
        .send_bytes()
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    let secret = key(
        &world,
        owner.id(),
        vec![rule("UploadSessionComplete", "upload", upload, "write")],
    )
    .await;
    let reply = world
        .app
        .call(UploadSessionComplete, &[&upload])
        .token(&secret)
        .send()
        .await
        .ok();
    assert_eq!(reply.size, 0);
    assert_eq!(reply.blob, Hash::of(b""));
    assert!(owns(&world, owner.id(), &reply.blob).await);
}

use orch_blob_contract::range::ByteRange;
use orch_blob_contract::store::{BlobError, BlobRead, BlobStore, ByteStream, Stored};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

struct PutGate {
    inner: Arc<dyn BlobStore>,
    directory: tempfile::TempDir,
    hash: Hash,
    puts: AtomicUsize,
    reached: Notify,
    release: Notify,
}

#[async_trait::async_trait]
impl BlobStore for PutGate {
    async fn put(
        &self,
        hash: &Hash,
        size: Option<i64>,
        body: ByteStream,
    ) -> Result<Stored, BlobError> {
        let stored = self.inner.put(hash, size, body).await?;
        if *hash == self.hash {
            assert_eq!(stored.meta.hash, self.hash);
            assert_eq!(stored.meta.size, 20);
            self.puts.fetch_add(1, Ordering::SeqCst);
            self.reached.notify_one();
            self.release.notified().await;
        }
        Ok(stored)
    }
    async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        self.inner.get(hash, range).await
    }
    async fn exists(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.inner.exists(hash).await
    }
    async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError> {
        self.inner.size(hash).await
    }
    async fn delete(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.inner.delete(hash).await
    }
}

fn gate_completion(world: &mut World) -> Arc<PutGate> {
    use orch_api_core::{registry::Registry, state::AppState, testing::TestApp};
    let old = world.app.app.clone();
    let gate = Arc::new(PutGate {
        inner: old.blobs.store(),
        directory: tempfile::tempdir().unwrap(),
        hash: Hash::of(COMPLETE_BYTES),
        puts: AtomicUsize::new(0),
        reached: Notify::new(),
        release: Notify::new(),
    });
    let state = Arc::new(AppState {
        db: old.db.clone(),
        config: old.config.clone(),
        clock: old.clock.clone(),
        link: old.link.clone(),
        engine: old.engine.clone(),
        realtime: old.realtime.clone(),
        build: old.build,
        blobs: orch_api_core::blobs::Blobs::new(gate.clone(), gate.directory.path()),
        cache: orch_api_core::cache::Cache::new(&old.config),
        host_ports: old.host_ports.clone(),
        builtin_sources: old.builtin_sources.clone(),
    });
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    world.app = TestApp::over(state, world.app.clock.clone(), registry);
    gate
}

#[tokio::test]
async fn upload_session_completion_rechecks_the_same_key_after_actual_native_store_io() {
    for expire in [false, true] {
        let mut world = World::new().await;
        let gate = gate_completion(&mut world);
        let owner = places::person(&world, "owner").await;
        let upload = filled(&world, &owner).await;
        let secret = key(
            &world,
            owner.id(),
            vec![rule("UploadSessionComplete", "upload", upload, "write")],
        )
        .await;
        let call = world
            .app
            .call(UploadSessionComplete, &[&upload])
            .token(&secret);
        let request = async move { call.send_bytes().await };
        let change = async {
            tokio::time::timeout(std::time::Duration::from_secs(30), gate.reached.notified())
                .await
                .expect("the native store must actually consume the selected complete file");
            assert!(
                !owns(&world, owner.id(), &gate.hash).await,
                "staging bytes grant no custody"
            );
            if expire {
                world.advance(Duration::hours(2));
            }
            gate.release.notify_one();
        };
        let (reply, ()) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            tokio::join!(request, change)
        })
        .await
        .unwrap();
        assert_eq!(
            reply.status,
            if expire {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::OK
            }
        );
        assert_eq!(
            gate.puts.load(Ordering::SeqCst),
            1,
            "guarded replanning never repeats native file IO"
        );
        assert_eq!(owns(&world, owner.id(), &gate.hash).await, !expire);
        let ended: bool =
            sqlx::query_scalar("SELECT completed_at IS NOT NULL FROM upload_sessions WHERE id=$1")
                .bind(upload)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(ended, !expire);
        assert_eq!(world.app.app.blobs.part_of_session(upload).exists(), expire);
        if expire {
            assert!(!String::from_utf8_lossy(&reply.raw).contains("private-session-name"));
        }
    }
}

#[tokio::test]
async fn upload_session_completion_rechecks_after_the_native_blob_catalogue_wait() {
    for expire in [false, true] {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let other = places::person(&world, "other").await;
        let hash = whole(&world, &other, COMPLETE_BYTES).await;
        let upload = filled(&world, &owner).await;
        let secret = key(
            &world,
            owner.id(),
            vec![rule("UploadSessionComplete", "upload", upload, "write")],
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
            let call = app.call(UploadSessionComplete, &[&upload]).token(&secret);
            call.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait, async {
            loop {
                assert!(!request.is_finished(), "the blob catalogue wait must be reached");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO blobs%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual blob catalogue serializes custody registration");
        if expire {
            world.advance(Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        assert_eq!(
            reply.status,
            if expire {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::OK
            }
        );
        assert_eq!(owns(&world, owner.id(), &hash).await, !expire);
        assert!(owns(&world, other.id(), &hash).await);
        let ended: bool =
            sqlx::query_scalar("SELECT completed_at IS NOT NULL FROM upload_sessions WHERE id=$1")
                .bind(upload)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(ended, !expire);
        assert_eq!(world.app.app.blobs.part_of_session(upload).exists(), expire);
    }
}

async fn stored_prefix(world: &World, upload: Id) -> Vec<u8> {
    tokio::fs::read(world.app.app.blobs.part_of_session(upload))
        .await
        .unwrap_or_default()
}
async fn received(world: &World, upload: Id) -> i64 {
    sqlx::query_scalar("SELECT received FROM upload_sessions WHERE id=$1")
        .bind(upload)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}
#[tokio::test]
async fn upload_piece_binds_the_real_session_keeps_native_offsets_and_needs_only_its_own_write() {
    let world = World::new().await;
    let owner = places::person(&world, "piece_owner").await;
    let other = places::person(&world, "piece_other").await;
    let upload = unfinished(&world, &owner).await;
    let another = unfinished(&world, &owner).await;
    let secret = key(
        &world,
        owner.id(),
        vec![rule("UploadSessionPut", "upload", upload, "write")],
    )
    .await;
    for (actor, selection) in [
        (
            owner.id(),
            rule("UploadSessionPut", "upload", another, "write"),
        ),
        (
            owner.id(),
            rule("UploadSessionPut", "upload", upload, "read"),
        ),
        (
            owner.id(),
            rule("UploadSessionComplete", "upload", upload, "write"),
        ),
        (
            other.id(),
            rule("UploadSessionPut", "upload", upload, "write"),
        ),
    ] {
        let refused = key(&world, actor, vec![selection]).await;
        let response = world
            .app
            .call(UploadSessionPut, &[&upload])
            .token(&refused)
            .query(&UploadSessionPutQuery { offset: 0 })
            .bytes(b"partial")
            .send_bytes()
            .await;
        assert!(matches!(
            response.status,
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        ));
        assert_eq!(received(&world, upload).await, 0);
        assert!(stored_prefix(&world, upload).await.is_empty());
    }
    let response = world
        .app
        .call(UploadSessionPut, &[&upload])
        .token(&secret)
        .query(&UploadSessionPutQuery { offset: 0 })
        .bytes(b"partial")
        .send_bytes()
        .await;
    assert_eq!(
        response.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&response.raw)
    );
    assert_eq!(
        serde_json::from_slice::<Json>(&response.raw).unwrap(),
        json!({"received":7})
    );
    let misplaced = world
        .app
        .call(UploadSessionPut, &[&upload])
        .token(&secret)
        .query(&UploadSessionPutQuery { offset: 0 })
        .bytes(b"wrong")
        .send_bytes()
        .await;
    assert_eq!(
        misplaced.problem().code,
        orch_api_contract::common::ErrorCode::UploadOffset
    );
    assert_eq!(misplaced.problem().extra["received"], json!(7));
    let negative = world
        .app
        .call(UploadSessionPut, &[&upload])
        .token(&secret)
        .query(&UploadSessionPutQuery { offset: -1 })
        .bytes(b"wrong")
        .send_bytes()
        .await;
    assert_eq!(
        negative.problem().code,
        orch_api_contract::common::ErrorCode::UploadOffset
    );
    assert_eq!(stored_prefix(&world, upload).await, b"partial");
    let next = world
        .app
        .call(UploadSessionPut, &[&upload])
        .token(&secret)
        .query(&UploadSessionPutQuery { offset: 7 })
        .bytes(b"tail")
        .send_bytes()
        .await;
    assert_eq!(next.status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Json>(&next.raw).unwrap(),
        json!({"received":11})
    );
    assert_eq!(received(&world, upload).await, 11);
    assert_eq!(stored_prefix(&world, upload).await, b"partialtail");
    assert!(!String::from_utf8_lossy(&next.raw).contains("private-session-name"));
}

#[tokio::test]
async fn upload_piece_checks_its_destination_before_reading_the_body_and_rechecks_the_key_after_body_wait()
 {
    use axum::body::Body;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tower::ServiceExt;
    let world = World::new().await;
    let owner = places::person(&world, "piece_body_owner").await;
    let upload = unfinished(&world, &owner).await;
    piece(&world, &owner, upload).await;
    let mut registry = orch_api_core::registry::Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = registry
        .into_router_for(orch_api_contract::surface::HttpSurface::Key)
        .with_state(world.app.app.clone());
    let denied = key(
        &world,
        owner.id(),
        vec![rule("UploadSessionPut", "upload", new_id(), "write")],
    )
    .await;
    let polls = Arc::new(AtomicUsize::new(0));
    let observed = polls.clone();
    let body = Body::from_stream(futures::stream::once(async move {
        observed.fetch_add(1, Ordering::SeqCst);
        Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"tail"))
    }));
    let response = router
        .clone()
        .oneshot(
            http::Request::builder()
                .method("PUT")
                .uri(format!("/uploads/sessions/{upload}?offset=7"))
                .header("authorization", format!("Bearer {denied}"))
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    let secret = key(
        &world,
        owner.id(),
        vec![rule("UploadSessionPut", "upload", upload, "write")],
    )
    .await;
    let (send, take) = tokio::sync::oneshot::channel::<bytes::Bytes>();
    let reached = Arc::new(tokio::sync::Notify::new());
    let notice = reached.clone();
    let body = Body::from_stream(futures::stream::once(async move {
        notice.notify_one();
        take.await.map_err(std::io::Error::other)
    }));
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("/uploads/sessions/{upload}?offset=7"))
        .header("authorization", format!("Bearer {secret}"))
        .body(body)
        .unwrap();
    let waiting = tokio::spawn(async move { router.oneshot(request).await.unwrap() });
    tokio::time::timeout(std::time::Duration::from_secs(30), reached.notified())
        .await
        .expect("the native bounded body was reached after destination admission");
    world.advance(Duration::hours(2));
    send.send(bytes::Bytes::from_static(b"tail")).unwrap();
    let response = tokio::time::timeout(std::time::Duration::from_secs(30), waiting)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(received(&world, upload).await, 7);
    assert_eq!(stored_prefix(&world, upload).await, b"partial");
}

#[tokio::test]
async fn upload_piece_rechecks_after_the_actual_session_row_wait_without_changing_the_existing_prefix()
 {
    for expire in [false, true] {
        let world = World::new().await;
        let owner = places::person(&world, "piece_lock_owner").await;
        let upload = unfinished(&world, &owner).await;
        piece(&world, &owner, upload).await;
        let secret = key(
            &world,
            owner.id(),
            vec![rule("UploadSessionPut", "upload", upload, "write")],
        )
        .await;
        let mut held = world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM upload_sessions WHERE id=$1 FOR UPDATE")
            .bind(upload)
            .execute(&mut *held)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *held)
            .await
            .unwrap();
        let app = world.app.clone();
        let mut pending = tokio::spawn(async move {
            let call = app
                .call(UploadSessionPut, &[&upload])
                .token(&secret)
                .query(&UploadSessionPutQuery { offset: 7 })
                .bytes(b"tail");
            call.send_bytes().await
        });
        let observed=tokio::time::timeout(std::time::Duration::from_secs(30),async {loop {
            if pending.is_finished(){break false;}
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM upload_sessions%FOR UPDATE%')").bind(pid).fetch_one(&world.db.pool).await.unwrap();if blocked{break true;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }}).await;
        if !matches!(observed, Ok(true)) {
            pending.abort();
            held.rollback().await.unwrap();
            let drained = pending.await;
            panic!("the native piece row wait was not reached: {drained:?}");
        }
        if expire {
            world.advance(Duration::hours(2));
        }
        held.rollback().await.unwrap();
        let response =
            match tokio::time::timeout(std::time::Duration::from_secs(30), &mut pending).await {
                Ok(result) => result.unwrap(),
                Err(error) => {
                    pending.abort();
                    let drained = pending.await;
                    panic!("the released native piece did not finish: {error}; {drained:?}")
                }
            };
        assert_eq!(
            response.status,
            if expire {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::OK
            }
        );
        assert_eq!(received(&world, upload).await, if expire { 7 } else { 11 });
        assert_eq!(
            stored_prefix(&world, upload).await,
            if expire {
                b"partial".as_slice()
            } else {
                b"partialtail".as_slice()
            }
        );
    }
}

#[tokio::test]
async fn completed_upload_piece_keeps_its_native_progress_receipt_after_activity_wait_and_key_expiry()
 {
    let world = World::new().await;
    let owner = places::person(&world, "piece_receipt_owner").await;
    let upload = unfinished(&world, &owner).await;
    piece(&world, &owner, upload).await;
    let secret = key(
        &world,
        owner.id(),
        vec![rule("UploadSessionPut", "upload", upload, "write")],
    )
    .await;
    let mut held = world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *held)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let app = world.app.clone();
    let mut pending = tokio::spawn(async move {
        let call = app
            .call(UploadSessionPut, &[&upload])
            .token(&secret)
            .query(&UploadSessionPutQuery { offset: 7 })
            .bytes(b"tail");
        call.send_bytes().await
    });
    let observed=tokio::time::timeout(std::time::Duration::from_secs(30),async {loop {
        if pending.is_finished(){break false;}let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')").bind(pid).fetch_one(&world.db.pool).await.unwrap();if blocked{break true;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }}).await;
    if !matches!(observed, Ok(true)) {
        pending.abort();
        held.rollback().await.unwrap();
        let drained = pending.await;
        panic!("the native completed-activity wait was not reached: {drained:?}");
    }
    assert_eq!(received(&world, upload).await, 11);
    assert_eq!(stored_prefix(&world, upload).await, b"partialtail");
    world.advance(Duration::hours(2));
    held.rollback().await.unwrap();
    let response =
        match tokio::time::timeout(std::time::Duration::from_secs(30), &mut pending).await {
            Ok(result) => result.unwrap(),
            Err(error) => {
                pending.abort();
                let drained = pending.await;
                panic!("the completed piece acknowledgement did not finish: {error}; {drained:?}")
            }
        };
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Json>(&response.raw).unwrap(),
        json!({"received":11})
    );
    assert_eq!(stored_prefix(&world, upload).await, b"partialtail");
}

fn attachment_rule(operation: &str, kind: &str, id: Id, action: &str, resource: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[resource],"allow":[{"operation":operation,"actions":[action]}]})
}
async fn reservation_count(site: &Site) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM thread_attachment_uploads WHERE thread_id=$1")
        .bind(site.thread)
        .fetch_one(&site.world.db.pool)
        .await
        .unwrap()
}
#[tokio::test]
async fn thread_attachment_direct_is_its_own_write_and_preserves_native_private_names_and_pins() {
    let site = Site::new().await;
    let secret = key(
        &site.world,
        site.ada.id(),
        vec![attachment_rule(
            "ThreadAttachmentCreate.direct",
            "thread",
            site.thread,
            "write",
            "thread",
        )],
    )
    .await;
    for selection in [
        attachment_rule(
            "ThreadAttachmentCreate.direct",
            "thread",
            new_id(),
            "write",
            "thread",
        ),
        attachment_rule(
            "ThreadAttachmentCreate.direct",
            "thread",
            site.thread,
            "read",
            "thread",
        ),
        attachment_rule(
            "ThreadInputCreate",
            "thread",
            site.thread,
            "write",
            "thread",
        ),
    ] {
        let wrong = key(&site.world, site.ada.id(), vec![selection]).await;
        let reply = site
            .app()
            .call(ThreadAttachmentCreate, &[&site.thread])
            .token(&wrong)
            .header(FILENAME_HEADER, "scoped.txt")
            .bytes(b"new direct bytes")
            .send_bytes()
            .await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert_eq!(reservation_count(&site).await, 0);
    }
    let reply = site
        .app()
        .call(ThreadAttachmentCreate, &[&site.thread])
        .token(&secret)
        .header(FILENAME_HEADER, "scoped.txt")
        .bytes(b"new direct bytes")
        .send_bytes()
        .await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let reply: ThreadAttachmentCreateReply = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(reply.path, ".orch/attachments/scoped.txt");
    assert_eq!(reply.blob, Hash::of(b"new direct bytes"));
    assert_eq!(reply.size, 16);
    assert!(!site.disk.exists(&reply.path));
    assert_eq!(reservation_count(&site).await, 1);
    let stored = storage::blob(&mut *site.world.conn().await, &reply.blob)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.pin_count, 1);
    let second = site
        .app()
        .call(ThreadAttachmentCreate, &[&site.thread])
        .token(&secret)
        .header(FILENAME_HEADER, "scoped.txt")
        .bytes(b"another body")
        .send_bytes()
        .await;
    assert_eq!(second.status, StatusCode::OK);
    let second: ThreadAttachmentCreateReply = serde_json::from_slice(&second.raw).unwrap();
    assert_ne!(
        second.path, reply.path,
        "native collision naming is preserved for write-only callers"
    );
}
#[tokio::test]
async fn thread_attachment_reference_uses_the_selected_native_upload_source_and_preserves_owned_replacement()
 {
    let site = Site::new().await;
    let blob = whole(&site.world, &site.ada, b"referenced bytes").await;
    let write = attachment_rule(
        "ThreadAttachmentCreate.from_upload",
        "thread",
        site.thread,
        "write",
        "thread",
    );
    let only_write = key(&site.world, site.ada.id(), vec![write.clone()]).await;
    let body = ThreadAttachmentBlobBody {
        blob,
        name: "reference.txt".into(),
        replace: None,
    };
    let denied = site
        .app()
        .call(ThreadAttachmentCreate, &[&site.thread])
        .token(&only_write)
        .raw_body(&serde_json::to_value(&body).unwrap())
        .send_bytes()
        .await;
    assert!(!denied.status.is_success());
    assert_eq!(reservation_count(&site).await, 0);
    let unrelated = key(
        &site.world,
        site.ada.id(),
        vec![
            write.clone(),
            attachment_rule("UploadBlobGet", "account", site.ada.id(), "read", "upload"),
        ],
    )
    .await;
    let denied = site
        .app()
        .call(ThreadAttachmentCreate, &[&site.thread])
        .token(&unrelated)
        .raw_body(&serde_json::to_value(&body).unwrap())
        .send_bytes()
        .await;
    assert!(
        !denied.status.is_success(),
        "an unrelated byte read does not lend this operation a source"
    );
    let secret = key(
        &site.world,
        site.ada.id(),
        vec![
            write,
            attachment_rule(
                "ThreadAttachmentCreate.from_upload",
                "account",
                site.ada.id(),
                "read",
                "upload",
            ),
        ],
    )
    .await;
    let first = site
        .app()
        .call(ThreadAttachmentCreate, &[&site.thread])
        .token(&secret)
        .raw_body(&serde_json::to_value(&body).unwrap())
        .send_bytes()
        .await;
    assert_eq!(
        first.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&first.raw)
    );
    let first: ThreadAttachmentCreateReply = serde_json::from_slice(&first.raw).unwrap();
    assert_eq!(first.blob, blob);
    let replacement = whole(&site.world, &site.ada, b"new replacement bytes").await;
    let body = ThreadAttachmentBlobBody {
        blob: replacement,
        name: "caller supplied alternate.txt".into(),
        replace: Some(first.path.clone()),
    };
    let changed = site
        .app()
        .call(ThreadAttachmentCreate, &[&site.thread])
        .token(&secret)
        .raw_body(&serde_json::to_value(body).unwrap())
        .send_bytes()
        .await;
    assert_eq!(changed.status, StatusCode::OK);
    let changed: ThreadAttachmentCreateReply = serde_json::from_slice(&changed.raw).unwrap();
    assert_eq!(changed.path, first.path);
    assert_eq!(changed.name, first.name);
    assert_eq!(changed.blob, replacement);
    assert_eq!(reservation_count(&site).await, 1);
    assert!(!site.disk.exists(&changed.path));
}

struct AttachmentSizeGate {
    inner: Arc<dyn BlobStore>,
    directory: tempfile::TempDir,
    hash: Hash,
    sizes: AtomicUsize,
    reached: Notify,
    release: Notify,
}
#[async_trait::async_trait]
impl BlobStore for AttachmentSizeGate {
    async fn put(
        &self,
        hash: &Hash,
        size: Option<i64>,
        body: ByteStream,
    ) -> Result<Stored, BlobError> {
        self.inner.put(hash, size, body).await
    }
    async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        self.inner.get(hash, range).await
    }
    async fn exists(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.inner.exists(hash).await
    }
    async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError> {
        if *hash == self.hash && self.sizes.fetch_add(1, Ordering::SeqCst) == 0 {
            self.reached.notify_one();
            self.release.notified().await;
        }
        self.inner.size(hash).await
    }
    async fn delete(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.inner.delete(hash).await
    }
}
fn gate_attachment_size(world: &mut World, hash: Hash) -> Arc<AttachmentSizeGate> {
    use orch_api_core::{registry::Registry, state::AppState, testing::TestApp};
    let old = world.app.app.clone();
    let gate = Arc::new(AttachmentSizeGate {
        inner: old.blobs.store(),
        directory: tempfile::tempdir().unwrap(),
        hash,
        sizes: AtomicUsize::new(0),
        reached: Notify::new(),
        release: Notify::new(),
    });
    let state = Arc::new(AppState {
        db: old.db.clone(),
        config: old.config.clone(),
        clock: old.clock.clone(),
        link: old.link.clone(),
        engine: old.engine.clone(),
        realtime: old.realtime.clone(),
        build: old.build,
        blobs: orch_api_core::blobs::Blobs::new(gate.clone(), gate.directory.path()),
        cache: orch_api_core::cache::Cache::new(&old.config),
        host_ports: old.host_ports.clone(),
        builtin_sources: old.builtin_sources.clone(),
    });
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    world.app = TestApp::over(state, world.app.clock.clone(), registry);
    gate
}
#[tokio::test]
async fn thread_attachment_reference_rechecks_its_original_key_after_the_native_source_size_wait() {
    for expire in [false, true] {
        let mut site = Site::new().await;
        let blob = whole(&site.world, &site.ada, b"source size wait bytes").await;
        let before = storage::blob(&mut *site.world.conn().await, &blob)
            .await
            .unwrap()
            .unwrap()
            .pin_count;
        let secret = key(
            &site.world,
            site.ada.id(),
            vec![
                attachment_rule(
                    "ThreadAttachmentCreate.from_upload",
                    "thread",
                    site.thread,
                    "write",
                    "thread",
                ),
                attachment_rule(
                    "ThreadAttachmentCreate.from_upload",
                    "account",
                    site.ada.id(),
                    "read",
                    "upload",
                ),
            ],
        )
        .await;
        let gate = gate_attachment_size(&mut site.world, blob);
        let thread = site.thread;
        let app = site.world.app.clone();
        let mut pending = tokio::spawn(async move {
            let call = app
                .call(ThreadAttachmentCreate, &[&thread])
                .token(&secret)
                .raw_body(
                    &serde_json::to_value(ThreadAttachmentBlobBody {
                        blob,
                        name: "size-wait.txt".into(),
                        replace: None,
                    })
                    .unwrap(),
                );
            call.send_bytes().await
        });
        let reached =
            tokio::time::timeout(std::time::Duration::from_secs(30), gate.reached.notified()).await;
        if reached.is_err() {
            pending.abort();
            gate.release.notify_one();
            let drained = pending.await;
            panic!("the actual referenced-source size wait was not reached: {drained:?}");
        }
        assert_eq!(reservation_count(&site).await, 0);
        if expire {
            site.world.advance(Duration::hours(2));
        }
        gate.release.notify_one();
        let response =
            match tokio::time::timeout(std::time::Duration::from_secs(30), &mut pending).await {
                Ok(result) => result.unwrap(),
                Err(error) => {
                    pending.abort();
                    let drained = pending.await;
                    panic!("the released reference did not finish: {error}; {drained:?}")
                }
            };
        assert_eq!(
            response.status,
            if expire {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::OK
            },
            "{}",
            String::from_utf8_lossy(&response.raw)
        );
        assert_eq!(reservation_count(&site).await, if expire { 0 } else { 1 });
        let after = storage::blob(&mut *site.world.conn().await, &blob)
            .await
            .unwrap()
            .unwrap()
            .pin_count;
        assert_eq!(after, if expire { before } else { before + 1 });
    }
}
#[tokio::test]
async fn thread_attachment_direct_checks_before_body_and_preserves_no_reservation_on_late_expiry() {
    use axum::body::Body;
    use tower::ServiceExt;
    let site = Site::new().await;
    let thread = site.thread;
    let mut registry = orch_api_core::registry::Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = registry
        .into_router_for(orch_api_contract::surface::HttpSurface::Key)
        .with_state(site.world.app.app.clone());
    let wrong = key(
        &site.world,
        site.ada.id(),
        vec![attachment_rule(
            "ThreadAttachmentCreate.direct",
            "thread",
            new_id(),
            "write",
            "thread",
        )],
    )
    .await;
    let polls = Arc::new(AtomicUsize::new(0));
    let observed = polls.clone();
    let body = Body::from_stream(futures::stream::once(async move {
        observed.fetch_add(1, Ordering::SeqCst);
        Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"direct bytes"))
    }));
    let response = router
        .clone()
        .oneshot(
            http::Request::builder()
                .method("POST")
                .uri(format!("/threads/{thread}/attachments"))
                .header("authorization", format!("Bearer {wrong}"))
                .header(FILENAME_HEADER, "body-wait.txt")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    let secret = key(
        &site.world,
        site.ada.id(),
        vec![attachment_rule(
            "ThreadAttachmentCreate.direct",
            "thread",
            thread,
            "write",
            "thread",
        )],
    )
    .await;
    let (send, take) = tokio::sync::oneshot::channel::<bytes::Bytes>();
    let reached = Arc::new(Notify::new());
    let notice = reached.clone();
    let body = Body::from_stream(futures::stream::once(async move {
        notice.notify_one();
        take.await.map_err(std::io::Error::other)
    }));
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/threads/{thread}/attachments"))
        .header("authorization", format!("Bearer {secret}"))
        .header(FILENAME_HEADER, "body-wait.txt")
        .body(body)
        .unwrap();
    let waiting = tokio::spawn(async move { router.oneshot(request).await.unwrap() });
    tokio::time::timeout(std::time::Duration::from_secs(30), reached.notified())
        .await
        .unwrap();
    site.world.advance(Duration::hours(2));
    send.send(bytes::Bytes::from_static(b"direct bytes"))
        .unwrap();
    let response = tokio::time::timeout(std::time::Duration::from_secs(30), waiting)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(reservation_count(&site).await, 0);
}

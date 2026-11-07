//! Raw file creation keeps native bytes, exact saved witnesses and current completed disclosure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod desk;
mod places;
mod support;

use desk::Desk;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::documents::{
    DocumentFileCreate, DocumentFileCreateQuery, NewDocumentAccess,
};
use orch_api_contract::threads::FILENAME_HEADER;
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_blob_contract::{
    range::ByteRange,
    store::{BlobError, BlobRead, BlobStore, ByteStream, Stored},
};
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Hash, Id, Json, new_id};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::Notify;

const DIRECT: &str = "DocumentFileCreate.direct";
const REFERENCE: &str = "DocumentFileCreate.from_upload";
// An existing fixture watchdog, never a native authority lifetime.
const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]})
}
fn upload(user: Id) -> Json {
    json!({"target":{"kind":"account","id":user,"reach":"self"},"resources":["upload"],"allow":[{"operation":REFERENCE,"actions":["read"]}]})
}
async fn key(desk: &Desk, user: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let key = Key {
        id: new_id(),
        secret: new_token(API_TOKEN_PREFIX),
    };
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: key.id,
            user_id: user,
            name: "Native file creation",
            token_hash: &token_hash(&key.secret),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: Some(desk.world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    key
}
async fn document(desk: &Desk, name: &str) -> Id {
    let mut body = desk::body(name);
    body.access = Some(NewDocumentAccess::Private);
    desk.make_body(&desk.ada, body).await.ok().base.id
}
async fn put(
    desk: &Desk,
    token: &str,
    document: Id,
    name: &str,
    bytes: &[u8],
    overwrite: bool,
) -> RawReply {
    let request = desk
        .world
        .app
        .call(DocumentFileCreate, &[&document])
        .token(token)
        .query(&DocumentFileCreateQuery {
            overwrite: Some(overwrite),
        })
        .header(FILENAME_HEADER, name)
        .bytes(bytes);
    request.send_bytes().await
}
async fn reference(desk: &Desk, token: &str, document: Id, hash: Hash, name: &str) -> RawReply {
    let request = desk
        .world
        .app
        .call(DocumentFileCreate, &[&document])
        .token(token)
        .query(&DocumentFileCreateQuery::default())
        .raw_body(&json!({"blob":hash,"name":name}));
    request.send_bytes().await
}
fn accepted(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn files(desk: &Desk, document: Id) -> Vec<(Id, Vec<u8>, String, String)> {
    sqlx::query_as(
        "SELECT id,blob,name,xmin::text FROM document_files WHERE document_id=$1 ORDER BY id",
    )
    .bind(document)
    .fetch_all(&desk.world.db.pool)
    .await
    .unwrap()
}
async fn state(desk: &Desk, document: Id) -> (String, i64, i64, i64) {
    sqlx::query_as("SELECT xmin::text,files_bytes,doc_seq,(SELECT count(*) FROM document_versions WHERE document_id=$1) FROM documents WHERE id=$1").bind(document).fetch_one(&desk.world.db.pool).await.unwrap()
}
async fn wait_sql(
    desk: &Desk,
    pid: i32,
    request: &tokio::task::JoinHandle<RawReply>,
    pattern: &str,
) {
    tokio::time::timeout(WAIT, async {
        loop {
            assert!(!request.is_finished(), "the actual native wait must be reached");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)").bind(pid).bind(pattern).fetch_one(&desk.world.db.pool).await.unwrap();
            if blocked { break; } tokio::task::yield_now().await;
        }
    }).await.expect("observe the actual query and literal blocker PID");
}

#[tokio::test]
async fn supplied_file_keeps_write_only_receipts_and_requires_its_own_typed_read_for_metadata() {
    let desk = Desk::new().await;
    let doc = document(&desk, "Private file target").await;
    for wrong in [
        rule("DocumentGet.read", "document", doc, &["write"]),
        rule(DIRECT, "document", doc, &["read"]),
        rule(DIRECT, "document_container", doc, &["write"]),
        rule(REFERENCE, "document", doc, &["write"]),
    ] {
        let key = key(&desk, desk.ada.id(), vec![wrong]).await;
        assert_eq!(
            put(&desk, &key.secret, doc, "denied.txt", b"new bytes", false)
                .await
                .status,
            StatusCode::FORBIDDEN
        );
        assert!(files(&desk, doc).await.is_empty());
    }
    let mut rules = vec![rule(DIRECT, "document", doc, &["write"])];
    rules.push(rule("DocumentGet.read", "document", doc, &["read"]));
    let writer = key(&desk, desk.ada.id(), rules).await;
    let reply = accepted(
        put(
            &desk,
            &writer.secret,
            doc,
            "new.txt",
            b"new file bytes",
            false,
        )
        .await,
    );
    let file: Id = serde_json::from_value(reply["file_id"].clone()).unwrap();
    assert_eq!(
        reply,
        json!({"projection":"receipt","document_id":doc,"file_id":file})
    );
    let actual = files(&desk, doc).await;
    assert_eq!(actual.len(), 1);
    assert_eq!(actual[0].0, file);
    assert_eq!(
        actual[0].1.as_slice(),
        Hash::of(b"new file bytes").as_bytes().as_slice()
    );
    let full = key(
        &desk,
        desk.ada.id(),
        vec![rule(DIRECT, "document", doc, &["write", "read"])],
    )
    .await;
    let reply = accepted(
        put(
            &desk,
            &full.secret,
            doc,
            "new.txt",
            b"replacement bytes",
            true,
        )
        .await,
    );
    assert_eq!(reply["id"], json!(file));
    assert_eq!(reply["name"], "new.txt");
    assert_eq!(reply["size"], 17);
    assert!(reply["added_by"].is_object());
    let outsider = desk.stranger("filenonviewer").await;
    let key = key(
        &desk,
        outsider.id(),
        vec![rule(DIRECT, "document", doc, &["write", "read"])],
    )
    .await;
    assert!(
        !put(
            &desk,
            &key.secret,
            doc,
            "private.txt",
            b"unauthorized",
            false
        )
        .await
        .status
        .is_success()
    );
    assert_eq!(files(&desk, doc).await.len(), 1);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hold {
    None,
    Put,
    Get,
}
struct Probe {
    inner: Arc<dyn BlobStore>,
    hash: Hash,
    hold: Hold,
    used: AtomicBool,
    puts: AtomicUsize,
    gets: AtomicUsize,
    sizes: AtomicUsize,
    reached: Notify,
    release: Notify,
}
impl Probe {
    async fn pause(&self, stage: Hold, hash: &Hash) {
        if *hash == self.hash && stage == self.hold && !self.used.swap(true, Ordering::SeqCst) {
            self.reached.notify_one();
            self.release.notified().await;
        }
    }
    async fn reached(&self) {
        tokio::time::timeout(WAIT, self.reached.notified())
            .await
            .expect("the actual native store supplier must wait");
    }
}
#[async_trait::async_trait]
impl BlobStore for Probe {
    async fn put(
        &self,
        hash: &Hash,
        size: Option<i64>,
        body: ByteStream,
    ) -> Result<Stored, BlobError> {
        if *hash == self.hash {
            self.puts.fetch_add(1, Ordering::SeqCst);
        }
        let stored = self.inner.put(hash, size, body).await;
        self.pause(Hold::Put, hash).await;
        stored
    }
    async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        if *hash == self.hash {
            self.gets.fetch_add(1, Ordering::SeqCst);
        }
        let read = self.inner.get(hash, range).await;
        self.pause(Hold::Get, hash).await;
        read
    }
    async fn exists(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.inner.exists(hash).await
    }
    async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError> {
        if *hash == self.hash {
            self.sizes.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.size(hash).await
    }
    async fn delete(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.inner.delete(hash).await
    }
}
fn probe(desk: &mut Desk, hash: Hash, hold: Hold) -> Arc<Probe> {
    use orch_api_core::{registry::Registry, state::AppState, testing::TestApp};
    let old = desk.world.app.app.clone();
    let probe = Arc::new(Probe {
        inner: old.blobs.store(),
        hash,
        hold,
        used: AtomicBool::new(false),
        puts: AtomicUsize::new(0),
        gets: AtomicUsize::new(0),
        sizes: AtomicUsize::new(0),
        reached: Notify::new(),
        release: Notify::new(),
    });
    let app = Arc::new(AppState {
        db: old.db.clone(),
        config: old.config.clone(),
        clock: old.clock.clone(),
        link: old.link.clone(),
        engine: old.engine.clone(),
        realtime: old.realtime.clone(),
        build: old.build,
        blobs: orch_api_core::blobs::Blobs::new(
            probe.clone(),
            std::path::Path::new(&old.config.storage.root),
        ),
        cache: orch_api_core::cache::Cache::new(&old.config),
        host_ports: old.host_ports.clone(),
        builtin_sources: old.builtin_sources.clone(),
    });
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    desk.world.app = TestApp::over(app, desk.world.app.clock.clone(), registry);
    probe
}

#[tokio::test]
async fn saved_file_sources_are_admitted_before_size_or_sniff_and_do_not_borrow_another_operation()
{
    let mut desk = Desk::new().await;
    let source = document(&desk, "Private saved bytes").await;
    let target = document(&desk, "Saved file target").await;
    let bytes = b"saved readable source bytes";
    accepted(put(&desk, &desk.ada.token, source, "source.txt", bytes, false).await);
    let hash = Hash::of(bytes);
    let probe = probe(&mut desk, hash, Hold::None);
    for extra in [
        Vec::new(),
        vec![rule("DocumentFileGet", "document", source, &["read"])],
    ] {
        let mut rules = vec![rule(REFERENCE, "document", target, &["write"])];
        rules.extend(extra);
        let key = key(&desk, desk.ada.id(), rules).await;
        assert_eq!(
            reference(&desk, &key.secret, target, hash, "copy.txt")
                .await
                .status,
            StatusCode::NOT_FOUND
        );
        assert_eq!(probe.sizes.load(Ordering::SeqCst), 0);
        assert_eq!(probe.gets.load(Ordering::SeqCst), 0);
        assert!(files(&desk, target).await.is_empty());
    }
    let editor = desk.member("filecopyeditor", "editor").await;
    let public_target = desk
        .make(&desk.ada, "Ordinary editor destination")
        .await
        .base
        .id;
    let inaccessible = key(
        &desk,
        editor.id(),
        vec![
            rule(REFERENCE, "document", public_target, &["write"]),
            rule(REFERENCE, "document", source, &["read"]),
        ],
    )
    .await;
    assert_eq!(
        reference(
            &desk,
            &inaccessible.secret,
            public_target,
            hash,
            "private.txt"
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        probe.sizes.load(Ordering::SeqCst),
        0,
        "scope grants no ordinary source visibility"
    );
    assert_eq!(probe.gets.load(Ordering::SeqCst), 0);
    assert!(files(&desk, public_target).await.is_empty());
    let source_reader = key(
        &desk,
        desk.ada.id(),
        vec![
            rule(REFERENCE, "document", target, &["write"]),
            rule(REFERENCE, "document", source, &["read"]),
        ],
    )
    .await;
    let copied = accepted(reference(&desk, &source_reader.secret, target, hash, "copy.txt").await);
    assert_eq!(copied["projection"], "receipt");
    assert_eq!(probe.sizes.load(Ordering::SeqCst), 1);
    assert_eq!(
        probe.gets.load(Ordering::SeqCst),
        1,
        "guarded replanning never sniffs again"
    );
    assert_eq!(probe.puts.load(Ordering::SeqCst), 0);
    let custody_reader = key(
        &desk,
        desk.ada.id(),
        vec![
            rule(REFERENCE, "document", target, &["write"]),
            upload(desk.ada.id()),
        ],
    )
    .await;
    accepted(reference(&desk, &custody_reader.secret, target, hash, "custody.txt").await);
    assert_eq!(files(&desk, target).await.len(), 2);
}

#[tokio::test]
async fn a_lost_selected_saved_witness_cannot_be_replaced_after_the_real_sniff_wait() {
    let mut desk = Desk::new().await;
    let source = document(&desk, "Alternative readable source").await;
    let target = document(&desk, "Frozen source target").await;
    let bytes = b"one immutable hash with two ordinary origins";
    accepted(put(&desk, &desk.ada.token, source, "source.txt", bytes, false).await);
    let hash = Hash::of(bytes);
    let key = key(
        &desk,
        desk.ada.id(),
        vec![
            rule(REFERENCE, "document", target, &["write"]),
            rule(REFERENCE, "document", source, &["read"]),
            upload(desk.ada.id()),
        ],
    )
    .await;
    let probe = probe(&mut desk, hash, Hold::Get);
    let before = state(&desk, target).await;
    let app = desk.world.app.clone();
    let secret = key.secret.clone();
    let request = tokio::spawn(async move {
        let request = app
            .call(DocumentFileCreate, &[&target])
            .token(&secret)
            .query(&DocumentFileCreateQuery::default())
            .raw_body(&json!({"blob":hash,"name":"frozen.txt"}));
        request.send_bytes().await
    });
    probe.reached().await;
    let removed = sqlx::query("DELETE FROM completed_uploads WHERE user_id=$1 AND hash=$2")
        .bind(desk.ada.id())
        .bind(hash.as_bytes().as_slice())
        .execute(&desk.world.db.pool)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(
        removed, 1,
        "the native preferred retained witness must actually exist"
    );
    probe.release.notify_one();
    let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(state(&desk, target).await, before);
    assert!(files(&desk, target).await.is_empty());
    assert_eq!(probe.sizes.load(Ordering::SeqCst), 1);
    assert_eq!(probe.gets.load(Ordering::SeqCst), 1);
    assert!(
        !orch_db::queries::uploads::owns(&mut *desk.world.conn().await, desk.ada.id(), &hash)
            .await
            .unwrap()
    );
    // The ordinary capability remains: a fresh request can select its still-readable Document.
    accepted(reference(&desk, &key.secret, target, hash, "fresh.txt").await);
}

#[tokio::test]
async fn saved_file_initial_io_rechecks_current_key_bounds_before_registering_custody() {
    for change in ["expired", "revoked", "narrowed"] {
        let mut desk = Desk::new().await;
        let source = document(&desk, "Initial file source").await;
        let target = document(&desk, "Initial file destination").await;
        let bytes = b"the initial supplier never grants a later write";
        accepted(put(&desk, &desk.ada.token, source, "source.txt", bytes, false).await);
        let hash = Hash::of(bytes);
        let token = key(
            &desk,
            desk.ada.id(),
            vec![
                rule(REFERENCE, "document", target, &["write"]),
                rule(REFERENCE, "document", source, &["read"]),
            ],
        )
        .await;
        let before = state(&desk, target).await;
        let source_before = files(&desk, source).await;
        let probe = probe(&mut desk, hash, Hold::Get);
        let app = desk.world.app.clone();
        let secret = token.secret.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(DocumentFileCreate, &[&target])
                .token(&secret)
                .query(&DocumentFileCreateQuery::default())
                .raw_body(&json!({"blob":hash,"name":"current.txt"}));
            request.send_bytes().await
        });
        probe.reached().await;
        if change == "expired" {
            desk.world.advance(chrono::Duration::hours(2));
        } else if change == "revoked" {
            assert!(
                identity::revoke_api_token(
                    &mut *desk.world.conn().await,
                    token.id,
                    desk.ada.id(),
                    desk.world.now()
                )
                .await
                .unwrap()
            );
        } else {
            let scope: KeyScope = serde_json::from_value(
                json!({"rules":[rule(REFERENCE, "document", target, &["write"])]}),
            )
            .unwrap();
            assert!(scope.validate().is_empty());
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(token.id)
                .bind(json!(scope))
                .execute(&desk.world.db.pool)
                .await
                .unwrap();
        }
        probe.release.notify_one();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        if change == "narrowed" {
            assert!(matches!(
                reply.status,
                StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
            ));
        } else {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        }
        assert_eq!(state(&desk, target).await, before);
        assert!(files(&desk, target).await.is_empty());
        assert_eq!(files(&desk, source).await, source_before);
        assert_eq!(probe.sizes.load(Ordering::SeqCst), 1);
        assert_eq!(probe.gets.load(Ordering::SeqCst), 1);
        assert_eq!(probe.puts.load(Ordering::SeqCst), 0);
        let pins: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
            .bind(hash.as_bytes().as_slice())
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
        assert_eq!(pins, 1, "only the original Document still pins the bytes");
    }
}

#[tokio::test]
async fn file_creation_rechecks_expiry_after_actual_body_document_and_blob_waits() {
    for stage in ["body", "document", "blob", "insert"] {
        let mut desk = Desk::new().await;
        let target = document(&desk, "First custody deadline").await;
        let bytes = b"same supplied bytes through the native body once";
        let hash = Hash::of(bytes);
        let key = key(
            &desk,
            desk.ada.id(),
            vec![rule(DIRECT, "document", target, &["write", "read"])],
        )
        .await;
        let before = state(&desk, target).await;
        let probe = probe(
            &mut desk,
            hash,
            if stage == "body" {
                Hold::Put
            } else {
                Hold::None
            },
        );
        if stage == "blob" {
            use orch_blob_contract::store::stream_of;
            probe
                .inner
                .put(
                    &hash,
                    Some(i64::try_from(bytes.len()).unwrap()),
                    stream_of(vec![bytes::Bytes::from_static(bytes)]),
                )
                .await
                .unwrap();
            orch_db::queries::storage::register_blob(
                &mut *desk.world.conn().await,
                &hash,
                i64::try_from(bytes.len()).unwrap(),
                desk.world.now(),
            )
            .await
            .unwrap();
        }
        let mut blocker = desk.world.db.pool.begin().await.unwrap();
        let pattern = if stage == "document" {
            sqlx::query("SELECT id FROM documents WHERE id=$1 FOR UPDATE")
                .bind(target)
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            "%FROM documents WHERE id%FOR UPDATE%"
        } else if stage == "blob" {
            sqlx::query("SELECT hash FROM blobs WHERE hash=$1 FOR UPDATE")
                .bind(hash.as_bytes().as_slice())
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            "%SELECT hash FROM blobs%FOR UPDATE%"
        } else if stage == "insert" {
            sqlx::query("LOCK TABLE document_files IN SHARE MODE")
                .execute(&mut *blocker)
                .await
                .unwrap();
            "%INSERT INTO document_files%"
        } else {
            ""
        };
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = desk.world.app.clone();
        let secret = key.secret.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(DocumentFileCreate, &[&target])
                .token(&secret)
                .query(&DocumentFileCreateQuery::default())
                .header(FILENAME_HEADER, "deadline.txt")
                .bytes(bytes);
            request.send_bytes().await
        });
        if stage == "body" {
            probe.reached().await;
        } else {
            wait_sql(&desk, pid, &request, pattern).await;
        }
        desk.world.advance(chrono::Duration::hours(2));
        if stage == "body" {
            probe.release.notify_one();
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        assert_eq!(
            reply.status,
            StatusCode::UNAUTHORIZED,
            "{stage}: {}",
            String::from_utf8_lossy(&reply.raw)
        );
        assert!(files(&desk, target).await.is_empty());
        assert_eq!(state(&desk, target).await, before);
        assert!(
            !orch_db::queries::uploads::owns(&mut *desk.world.conn().await, desk.ada.id(), &hash)
                .await
                .unwrap()
        );
        assert_eq!(
            probe.puts.load(Ordering::SeqCst),
            1,
            "the body is never replayed"
        );
        let pins: Option<i32> = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
            .bind(hash.as_bytes().as_slice())
            .fetch_optional(&desk.world.db.pool)
            .await
            .unwrap();
        assert_eq!(pins.unwrap_or(0), 0);
    }
}

#[tokio::test]
async fn completed_file_publication_keeps_a_truthful_receipt_without_repeating_the_writer() {
    for change in [
        "none",
        "original_read_closed",
        "result_read_closed",
        "native_view_closed",
        "expired",
        "replaced",
    ] {
        let desk = Desk::new().await;
        let source = document(&desk, "Completed file source").await;
        let target = document(&desk, "Completed file target").await;
        let bytes = b"committed file bytes";
        accepted(put(&desk, &desk.ada.token, source, "source.txt", bytes, false).await);
        let hash = Hash::of(bytes);
        let token = key(
            &desk,
            desk.ada.id(),
            vec![
                rule(REFERENCE, "document", source, &["read"]),
                rule(REFERENCE, "document", target, &["write", "read"]),
            ],
        )
        .await;
        let new_owner = desk.stranger("filepublicationowner").await;
        let mut blocker = desk.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = desk.world.app.clone();
        let secret = token.secret.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(DocumentFileCreate, &[&target])
                .token(&secret)
                .query(&DocumentFileCreateQuery::default())
                .raw_body(&json!({"blob":hash,"name":"held.txt"}));
            request.send_bytes().await
        });
        wait_sql(&desk, pid, &request, "%INSERT INTO api_token_activity%").await;
        let before = files(&desk, target).await;
        assert_eq!(before.len(), 1);
        let file = before[0].0;
        if change == "expired" {
            desk.world.advance(chrono::Duration::hours(2));
        }
        if change == "native_view_closed" {
            let mut changed = desk.world.db.pool.begin().await.unwrap();
            let affected = sqlx::query("UPDATE permission_entries SET holder_id=$2 WHERE object_kind='document' AND object_id=$1 AND permission='access.own' AND holder_id=$3")
                .bind(target).bind(new_owner.id()).bind(desk.ada.id())
                .execute(&mut *changed).await.unwrap().rows_affected();
            assert!(affected > 0);
            orch_db::queries::access::bump_epoch(&mut changed)
                .await
                .unwrap();
            changed.commit().await.unwrap();
        }
        if change == "original_read_closed" || change == "result_read_closed" {
            let mut rules = vec![rule(
                REFERENCE,
                "document",
                target,
                if change == "result_read_closed" {
                    &["write"]
                } else {
                    &["write", "read"]
                },
            )];
            if change == "result_read_closed" {
                rules.push(rule(REFERENCE, "document", source, &["read"]));
            }
            let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
            assert!(scope.validate().is_empty());
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(token.id)
                .bind(json!(scope))
                .execute(&desk.world.db.pool)
                .await
                .unwrap();
        }
        if change == "replaced" {
            accepted(
                put(
                    &desk,
                    &desk.ada.token,
                    target,
                    "held.txt",
                    b"later native replacement",
                    true,
                )
                .await,
            );
        }
        let expected = files(&desk, target).await;
        let material = state(&desk, target).await;
        blocker.commit().await.unwrap();
        let reply = accepted(tokio::time::timeout(WAIT, request).await.unwrap().unwrap());
        if change == "none" || change == "original_read_closed" {
            assert_eq!(reply["id"], json!(file));
            assert_eq!(reply["name"], "held.txt");
        } else {
            assert_eq!(
                reply,
                json!({"projection":"receipt","document_id":target,"file_id":file})
            );
        }
        assert_eq!(files(&desk, target).await, expected);
        assert_eq!(
            state(&desk, target).await,
            material,
            "publication cannot write, pin, touch or append history again"
        );
    }
}

#[tokio::test]
async fn file_conflict_and_limit_refusals_disclose_only_their_own_declared_current_sources() {
    let desk = Desk::new().await;
    let target = document(&desk, "Refusal target").await;
    let first = accepted(put(&desk, &desk.ada.token, target, "same.txt", b"before", false).await);
    let before = files(&desk, target).await;
    for read in [false, true] {
        let writer = key(
            &desk,
            desk.ada.id(),
            vec![rule(
                DIRECT,
                "document",
                target,
                if read { &["write", "read"] } else { &["write"] },
            )],
        )
        .await;
        let reply = put(&desk, &writer.secret, target, "same.txt", b"refused", false).await;
        assert_eq!(reply.status, StatusCode::CONFLICT);
        let problem: Json = serde_json::from_slice(&reply.raw).unwrap();
        if read {
            assert_eq!(problem["existing_id"], first["id"]);
        } else {
            assert!(problem.get("existing_id").is_none());
        }
        assert_eq!(files(&desk, target).await, before);
    }
    desk.world
        .set_at(
            orch_settings::Target::Node(desk.topic),
            "docs.file_max_bytes",
            json!(8),
        )
        .await;
    for named in [false, true] {
        let mut rules = vec![rule(DIRECT, "document", target, &["write"])];
        if named {
            rules.push(rule(DIRECT, "topic", desk.topic, &["read"]));
        }
        let writer = key(&desk, desk.ada.id(), rules).await;
        let reply = put(
            &desk,
            &writer.secret,
            target,
            "too-big.txt",
            b"longer than eight bytes",
            false,
        )
        .await;
        assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
        let problem: Json = serde_json::from_slice(&reply.raw).unwrap();
        assert_eq!(problem["code"], "docs.too_large");
        assert_eq!(problem["value"], 8);
        if named {
            assert_eq!(problem["scope"]["id"], json!(desk.topic));
        } else {
            assert!(problem["scope"].is_null());
        }
        assert_eq!(files(&desk, target).await, before);
    }
}

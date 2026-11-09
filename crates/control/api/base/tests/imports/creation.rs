//! The actual once-opened input, resolved native variant and every new native write stay bounded.
use super::*;
use async_trait::async_trait;
use orch_api_contract::imports::{ImportAuthors, ImportCreationReceipt, ImportCreationResponse};
use orch_blob_contract::{
    range::ByteRange,
    store::{BlobError, BlobRead, BlobStore, ByteStream, Stored},
};
use orch_db::queries::jobs::{self, JobRow};
use orch_types::Hash;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::Notify;

const CONTENT: &str = "NodeImportCreate.content";
const RESTORE: &str = "NodeImportCreate.restore_access";
fn source(o: &Office, op: &str) -> Value {
    rule(op, "account", o.s.ada.id(), "self", &["upload"], &["read"])
}
fn destination(o: &Office, op: &str, manage: bool) -> Value {
    rule(
        op,
        "topic",
        o.topic(),
        "self",
        &["document"],
        if manage {
            &["write", "manage"]
        } else {
            &["write"]
        },
    )
}
fn rules(o: &Office, op: &str, manage: bool) -> Vec<Value> {
    vec![source(o, op), destination(o, op, manage)]
}
async fn body(o: &Office, name: &str) -> ImportRequest {
    let file = upload(
        o,
        &o.s.ada,
        name,
        b"# Scoped native import

Only its actual source is copied.
",
    )
    .await;
    let mut body = asked(&new_id().to_string(), &file);
    body.options.authors = Some(ImportAuthors::Labels);
    body.options.access = Some(false);
    body
}
async fn create(o: &Office, token: &str, body: &ImportRequest) -> RawReply {
    let node = o.topic();
    let call = o.s.rig.world.app.call(NodeImportCreate, &[&node]);
    call.token(token).body(body).send_bytes().await
}
fn receipt(reply: RawReply) -> ImportCreationReceipt {
    assert_eq!(
        reply.status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: Value = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 3);
    match serde_json::from_value::<ImportCreationResponse>(value).unwrap() {
        ImportCreationResponse::Receipt(receipt) => receipt,
        ImportCreationResponse::Full(_) => {
            panic!("a source/effect grant does not lend private job metadata Read")
        }
    }
}
async fn state(o: &Office, id: Id) -> JobRow {
    jobs::job(&mut *o.s.rig.world.conn().await, id)
        .await
        .unwrap()
        .unwrap()
}
async fn ended(o: &Office, id: Id) -> JobRow {
    rig::eventually("the actual scoped native import ends", || async {
        !state(o, id).await.is_open()
    })
    .await;
    state(o, id).await
}
async fn counts(o: &Office) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM imports),(SELECT count(*) FROM jobs WHERE kind='import'),(SELECT count(*) FROM documents)").fetch_one(o.s.rig.db.pool()).await.unwrap()
}
#[tokio::test]
async fn creation_requires_exact_selected_source_and_native_destination_and_returns_real_ids_only()
{
    let o = Office::new().await;
    let body = body(&o, "scoped-new.md").await;
    let before = counts(&o).await;
    for scope in [
        vec![source(&o, CONTENT)],
        vec![destination(&o, CONTENT, false)],
        vec![
            source(&o, CONTENT),
            rule(
                CONTENT,
                "topic",
                o.topic(),
                "self",
                &["document"],
                &["read"],
            ),
        ],
        vec![
            source(&o, CONTENT),
            rule(
                CONTENT,
                "project",
                o.topic(),
                "self",
                &["document"],
                &["write"],
            ),
        ],
        vec![
            source(&o, "NodeImportPreflight"),
            destination(&o, CONTENT, false),
        ],
        rules(&o, RESTORE, true),
    ] {
        let denied = key(&o, &o.s.ada, scope).await;
        let reply = create(&o, &denied.secret, &body).await;
        assert!(
            reply.status.is_client_error(),
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        assert_eq!(counts(&o).await, before);
    }
    let writer = key(&o, &o.s.ada, rules(&o, CONTENT, false)).await;
    let accepted = receipt(create(&o, &writer.secret, &body).await);
    let stored = state(&o, accepted.id).await;
    assert_eq!(stored.plan["import"], json!(accepted.import_id));
    assert_eq!(stored.plan["import_operation_required"], true);
    assert_eq!(
        stored.plan["import_operation"]["http"]["credential"]["kind"],
        "token"
    );
    assert_eq!(
        stored.plan["import_operation"]["http"]["credential"]["id"],
        json!(writer.id)
    );
    let done = ended(&o, accepted.id).await;
    assert_eq!(done.state, "succeeded", "{:?}", done.error);
    assert_eq!(counts(&o).await, (before.0 + 1, before.1 + 1, before.2 + 1));
    let made = read(&o, &o.s.ada, accepted.import_id).await;
    assert_eq!(
        made.request, body,
        "ordinary stored request meaning stays unchanged"
    );
}
#[tokio::test]
async fn the_native_default_selects_restore_accounts_and_cannot_be_relabelled_by_a_content_grant() {
    let o = Office::new().await;
    let mut body = body(&o, "native-default.md").await;
    body.options.authors = None;
    body.options.access = None;
    let native = plan(&o, &o.s.ada, o.topic(), &body).await.ok();
    assert_eq!(
        native.authors,
        ImportAuthors::Accounts,
        "this actual root owner receives the native administrator default"
    );
    let before = counts(&o).await;
    for (op, manage) in [(CONTENT, true), (RESTORE, false)] {
        let denied = key(&o, &o.s.ada, rules(&o, op, manage)).await;
        assert!(
            create(&o, &denied.secret, &body)
                .await
                .status
                .is_client_error()
        );
        assert_eq!(counts(&o).await, before);
    }
    let exact = key(&o, &o.s.ada, rules(&o, RESTORE, true)).await;
    let accepted = receipt(create(&o, &exact.secret, &body).await);
    let stored = state(&o, accepted.id).await;
    assert_eq!(stored.plan["import_operation"]["authors"], "accounts");
    assert_eq!(ended(&o, accepted.id).await.state, "succeeded");
    let rows = report(&o, &o.s.ada, accepted.import_id).await;
    let documents = of_kind(&rows, PackageKind::Document);
    assert_eq!(documents.len(), 1);
    let document = documents[0].made.as_ref().unwrap().id;
    body.client_id = new_id().to_string();
    body.options.authors = Some(ImportAuthors::Labels);
    body.options.access = Some(false);
    body.options.same_source = Some(ImportAction::Replace);
    // This fresh import replaces the first document, so its stored governor is required.
    let before = counts(&o).await;
    let exact = key(&o, &o.s.ada, rules(&o, CONTENT, false)).await;
    assert_eq!(
        create(&o, &exact.secret, &body).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(counts(&o).await, before);
    let mut grants = rules(&o, CONTENT, false);
    grants.push(rule(
        CONTENT,
        "document",
        document,
        "self",
        &["document"],
        &["write"],
    ));
    let exact = key(&o, &o.s.ada, grants).await;
    let accepted = receipt(create(&o, &exact.secret, &body).await);
    assert_eq!(ended(&o, accepted.id).await.state, "succeeded");
    assert_eq!(counts(&o).await, (before.0 + 1, before.1 + 1, before.2));
}
#[tokio::test]
async fn the_full_native_job_needs_this_operations_actual_creator_read_and_replay_keeps_the_first_origin()
 {
    let o = Office::new().await;
    let mut body = body(&o, "job-reader.md").await;
    body.options.same_source = Some(ImportAction::Replace);
    let act = new_id().to_string();
    let mut wrong = rules(&o, CONTENT, false);
    wrong.push(rule(
        "UserGet",
        "account",
        o.s.ada.id(),
        "self",
        &["account"],
        &["read"],
    ));
    let writer = key(&o, &o.s.ada, wrong).await;
    let accepted = receipt(retry(&o, &writer.secret, &act, &body).await);
    assert_eq!(ended(&o, accepted.id).await.state, "succeeded");
    let original = state(&o, accepted.id).await.plan["import_operation"].clone();
    let mut exact = rules(&o, CONTENT, false);
    exact.push(rule(
        CONTENT,
        "account",
        o.s.ada.id(),
        "self",
        &["account"],
        &["read"],
    ));
    let reader = key(&o, &o.s.ada, exact).await;
    let before = counts(&o).await;
    // A new attempt still needs the current document governor, not the spent creation grant.
    let fresh = retry(&o, &reader.secret, &new_id().to_string(), &body).await;
    assert_eq!(fresh.status, StatusCode::FORBIDDEN);
    assert_eq!(counts(&o).await, before);
    // Completed readback uses the first action's key with this credential's own Read.
    let reply = retry(&o, &reader.secret, &act, &body).await;
    assert_eq!(
        reply.status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let job = serde_json::from_slice::<ImportCreationResponse>(&reply.raw)
        .unwrap()
        .into_full()
        .unwrap();
    assert_eq!(job.id, accepted.id);
    assert_eq!(job.created_by.as_ref().unwrap().id, o.s.ada.id());
    assert_eq!(counts(&o).await, before);
    assert_eq!(
        state(&o, accepted.id).await.plan["import_operation"],
        original,
        "repeating a request never replaces its first raw credential"
    );
}
async fn wait(o: &Office, pid: i32, job: Id) {
    tokio::time::timeout(rig::PATIENCE,async {
        loop {
            assert!(state(o,job).await.is_open(),"the actual writer must reach the document row");
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM documents%FOR UPDATE%')").bind(pid).fetch_one(o.s.rig.db.pool()).await.unwrap();
            if waiting {break;}tokio::task::yield_now().await;
        }
    }).await.expect("the native document source/write lock");
}
#[tokio::test]
async fn real_document_waits_recheck_key_scope_expiry_placement_and_required_origin_before_any_rewrite()
 {
    for mode in [
        "unchanged",
        "scope_lost",
        "expired",
        "rehome",
        "missing",
        "null",
        "cross_job",
    ] {
        let o = Office::new().await;
        let mut body = body(&o, "Handbook.md").await;
        body.options.same_name = Some(ImportAction::Replace);
        let before: i64 = sqlx::query_scalar("SELECT doc_seq FROM documents WHERE id=$1")
            .bind(o.document.base.id)
            .fetch_one(o.s.rig.db.pool())
            .await
            .unwrap();
        let other = places::make_org(
            &o.s.rig.world,
            &o.s.ada,
            "Another import home",
            "other-import",
        )
        .await
        .base
        .id;
        let writer = key(
            &o,
            &o.s.ada,
            vec![
                source(&o, CONTENT),
                rule(
                    CONTENT,
                    "document",
                    o.document.base.id,
                    "self",
                    &["document"],
                    &["write"],
                ),
            ],
        )
        .await;
        let mut blocker = o.s.rig.db.pool().begin().await.unwrap();
        sqlx::query("SELECT id FROM documents WHERE id=$1 FOR UPDATE")
            .bind(o.document.base.id)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let accepted = receipt(create(&o, &writer.secret, &body).await);
        wait(&o, pid, accepted.id).await;
        match mode {
            "scope_lost" => {
                let scope: KeyScope =
                    serde_json::from_value(json!({"rules":[source(&o,CONTENT)]})).unwrap();
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(writer.id)
                    .bind(json!(scope))
                    .execute(o.s.rig.db.pool())
                    .await
                    .unwrap();
            }
            "expired" => o.s.rig.world.advance(chrono::Duration::hours(2)),
            "rehome" => {
                let moved = sqlx::query("UPDATE documents AS d SET node_id=n.id,org_id=n.org_id,parent_container_id=NULL FROM nodes n WHERE d.id=$1 AND n.id=$2")
                    .bind(o.document.base.id)
                    .bind(other)
                    .execute(&mut *blocker)
                    .await
                    .unwrap();
                assert_eq!(moved.rows_affected(), 1);
            }
            "missing" | "null" | "cross_job" => {
                let mut value = state(&o, accepted.id).await.plan;
                match mode {
                    "missing" => {
                        value.as_object_mut().unwrap().remove("import_operation");
                    }
                    "null" => value["import_operation"] = Value::Null,
                    _ => value["import_operation"]["job"] = json!(new_id()),
                }
                sqlx::query("UPDATE jobs SET plan=$2 WHERE id=$1")
                    .bind(accepted.id)
                    .bind(value)
                    .execute(o.s.rig.db.pool())
                    .await
                    .unwrap();
            }
            _ => {}
        }
        blocker.commit().await.unwrap();
        let done = ended(&o, accepted.id).await;
        let after: i64 = sqlx::query_scalar("SELECT doc_seq FROM documents WHERE id=$1")
            .bind(o.document.base.id)
            .fetch_one(o.s.rig.db.pool())
            .await
            .unwrap();
        if mode == "unchanged" {
            assert_eq!(done.state, "succeeded", "{:?}", done.error);
            assert!(after > before);
        } else {
            assert_eq!(after, before, "{mode}: {:?}", done.error);
        }
    }
}

/// The real archive opener waits once, before any queue row can exist.
struct OpeningStore {
    inner: Arc<dyn BlobStore>,
    armed: AtomicBool,
    reads: AtomicUsize,
    reached: Notify,
    release: Notify,
}
#[async_trait]
impl BlobStore for OpeningStore {
    async fn put(
        &self,
        hash: &Hash,
        size: Option<i64>,
        content: ByteStream,
    ) -> Result<Stored, BlobError> {
        self.inner.put(hash, size, content).await
    }
    async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        if self.armed.load(Ordering::SeqCst) && self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
            self.reached.notify_one();
            self.release.notified().await;
        }
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
async fn opening_office() -> (Office, Arc<OpeningStore>) {
    let slot = Arc::new(std::sync::OnceLock::new());
    let capture = slot.clone();
    let office = Office::with_blob_layer(Box::new(move |inner| {
        let store = Arc::new(OpeningStore {
            inner,
            armed: AtomicBool::new(false),
            reads: AtomicUsize::new(0),
            reached: Notify::new(),
            release: Notify::new(),
        });
        assert!(capture.set(store.clone()).is_ok());
        store
    }))
    .await;
    (office, slot.get().unwrap().clone())
}

#[tokio::test]
async fn the_real_once_only_opener_keeps_original_scope_and_raw_expiry_before_queueing() {
    for mode in [
        "unchanged",
        "source_lost",
        "write_lost",
        "original_expiry",
        "person_suspended",
    ] {
        let (o, store) = opening_office().await;
        let body = body(&o, "opening.md").await;
        let writer = key(&o, &o.s.ada, rules(&o, CONTENT, false)).await;
        store.armed.store(true, Ordering::SeqCst);
        let before = counts(&o).await;
        let app = o.s.rig.world.app.clone();
        let node = o.topic();
        let token = writer.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(NodeImportCreate, &[&node]);
            call.token(&token).body(&body).send_bytes().await
        });
        tokio::time::timeout(rig::PATIENCE, store.reached.notified())
            .await
            .expect("the native archive is opened");
        assert_eq!(store.reads.load(Ordering::SeqCst), 1);
        assert_eq!(
            counts(&o).await,
            before,
            "fetching source bytes is effect-free"
        );
        match mode {
            "source_lost" | "write_lost" => {
                let retained = if mode == "source_lost" {
                    destination(&o, CONTENT, false)
                } else {
                    source(&o, CONTENT)
                };
                let scope: KeyScope = serde_json::from_value(json!({"rules":[retained]})).unwrap();
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(writer.id)
                    .bind(json!(scope))
                    .execute(o.s.rig.db.pool())
                    .await
                    .unwrap();
            }
            "original_expiry" => {
                sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                    .bind(writer.id)
                    .bind(o.s.rig.world.now() + chrono::Duration::hours(8))
                    .execute(o.s.rig.db.pool())
                    .await
                    .unwrap();
                o.s.rig.world.advance(chrono::Duration::hours(2));
            }
            "person_suspended" => {
                sqlx::query("UPDATE users SET status='suspended' WHERE id=$1")
                    .bind(o.s.ada.id())
                    .execute(o.s.rig.db.pool())
                    .await
                    .unwrap();
            }
            _ => {}
        }
        store.release.notify_one();
        let reply = tokio::time::timeout(rig::PATIENCE, request)
            .await
            .unwrap()
            .unwrap();
        if mode == "unchanged" {
            let accepted = receipt(reply);
            assert_eq!(ended(&o, accepted.id).await.state, "succeeded");
            assert_eq!(counts(&o).await, (before.0 + 1, before.1 + 1, before.2 + 1));
        } else {
            assert!(
                reply.status.is_client_error(),
                "{mode}: {}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(counts(&o).await, before);
            assert_eq!(
                store.reads.load(Ordering::SeqCst),
                1,
                "pure re-preparation never opens the same archive again"
            );
        }
    }
}
#[tokio::test]
async fn an_exact_scope_never_supplies_someone_elses_ordinary_upload_custody() {
    let (o, store) = opening_office().await;
    let body = body(&o, "custody.md").await;
    let stranger = key(&o, &o.s.cai, rules(&o, CONTENT, false)).await;
    store.armed.store(true, Ordering::SeqCst);
    let before = counts(&o).await;
    let reply = create(&o, &stranger.secret, &body).await;
    assert!(reply.status.is_client_error());
    assert_eq!(
        store.reads.load(Ordering::SeqCst),
        0,
        "scope does not open bytes that native custody refuses"
    );
    assert_eq!(counts(&o).await, before);
}

#[tokio::test]
async fn native_new_folders_use_the_actual_existing_creation_governor_and_resume_their_children() {
    let o = Office::new().await;
    let first = upload(&o, &o.s.ada, "notes/first.md", b"# First\n").await;
    let second = upload(&o, &o.s.ada, "notes/second.md", b"# Second\n").await;
    let mut body = asked(&new_id().to_string(), &first);
    body.source.files.push(second);
    body.options.authors = Some(ImportAuthors::Labels);
    body.options.access = Some(false);
    let scope = vec![
        source(&o, CONTENT),
        rule(
            CONTENT,
            "topic",
            o.topic(),
            "self",
            &["document_container", "document"],
            &["write"],
        ),
    ];
    let writer = key(&o, &o.s.ada, scope).await;
    let accepted = receipt(create(&o, &writer.secret, &body).await);
    assert_eq!(ended(&o, accepted.id).await.state, "succeeded");
    let rows = report(&o, &o.s.ada, accepted.import_id).await;
    assert_eq!(of_kind(&rows, PackageKind::Folder).len(), 1);
    assert_eq!(of_kind(&rows, PackageKind::Document).len(), 2);
    assert!(
        rows.iter().all(|row| row.outcome != ImportOutcome::Failed),
        "{rows:?}"
    );
}

#[tokio::test]
async fn the_native_queue_cut_runs_before_its_first_pin_and_can_use_the_same_prepared_input() {
    let o = Office::new().await;
    let body = body(&o, "queue-cut.md").await;
    let caller = orch_authz::Caller::session(o.s.ada.id());
    let prepared =
        o.s.rig
            .engine
            .prepare_import(&caller, o.topic(), body.clone())
            .await
            .unwrap();
    let mut tx = o.s.rig.db.pool().begin().await.unwrap();
    let queued =
        o.s.rig
            .engine
            .prepare_import_queue(&mut tx, &caller, prepared)
            .await
            .unwrap();
    let pins: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(body.source.files[0].blob.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let denied =
        o.s.rig
            .engine
            .apply_import_queue_before(&mut tx, &caller, queued.clone(), || {
                Err::<(), _>(orch_engine::EngineError::unreadable(
                    "the original request ended",
                ))
            })
            .await;
    assert!(denied.is_err());
    let after: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(body.source.files[0].blob.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        after, pins,
        "even the first rollbackable native effect has not happened"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM imports WHERE user_id=$1")
        .bind(o.s.ada.id())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='import' AND created_by=$1")
            .bind(o.s.ada.id())
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(count, 0);
    let checked = std::sync::atomic::AtomicBool::new(false);
    let made =
        o.s.rig
            .engine
            .apply_import_queue_before(&mut tx, &caller, queued, || {
                checked.store(true, Ordering::SeqCst);
                Ok::<(), orch_engine::EngineError>(())
            })
            .await
            .unwrap();
    assert!(checked.load(Ordering::SeqCst));
    assert_eq!(made.import.job_id, Some(made.job.id));
    tx.rollback().await.unwrap();
}

async fn retry(o: &Office, secret: &str, act: &str, body: &ImportRequest) -> RawReply {
    let node = o.topic();
    let call = o.s.rig.world.app.call(NodeImportCreate, &[&node]);
    call.token(secret)
        .header("Idempotency-Key", act)
        .body(body)
        .send_bytes()
        .await
}
#[tokio::test]
async fn creation_replay_reads_the_current_job_without_reopening_or_reimporting_its_source() {
    let (o, store) = opening_office().await;
    let body = body(&o, "one-header-import.md").await;
    let act = new_id().to_string();
    store.armed.store(true, Ordering::SeqCst);
    store.reads.store(1, Ordering::SeqCst);
    let writer = key(&o, &o.s.ada, rules(&o, CONTENT, false)).await;
    let accepted = receipt(retry(&o, &writer.secret, &act, &body).await);
    assert_eq!(ended(&o, accepted.id).await.state, "succeeded");
    let before = counts(&o).await;
    let reads = store.reads.load(Ordering::SeqCst);
    let origin = state(&o, accepted.id).await.plan["import_operation"].clone();
    let reader = key(
        &o,
        &o.s.ada,
        vec![rule(
            CONTENT,
            "account",
            o.s.ada.id(),
            "self",
            &["account"],
            &["read"],
        )],
    )
    .await;
    let response = retry(&o, &reader.secret, &act, &body).await;
    assert_eq!(
        response.status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&response.raw)
    );
    let current: ImportCreationResponse = serde_json::from_slice(&response.raw).unwrap();
    let current = current.into_full().unwrap();
    assert_eq!(current.id, accepted.id);
    assert_eq!(current.state, JobState::Succeeded);
    let wrong = key(
        &o,
        &o.s.ada,
        vec![rule(
            "UserGet",
            "account",
            o.s.ada.id(),
            "self",
            &["account"],
            &["read"],
        )],
    )
    .await;
    assert_eq!(
        receipt(retry(&o, &wrong.secret, &act, &body).await),
        accepted
    );
    assert_eq!(
        store.reads.load(Ordering::SeqCst),
        reads,
        "completed replay never reopens consumed source bytes"
    );
    assert!(
        retry(&o, &reader.secret, &new_id().to_string(), &body)
            .await
            .status
            .is_client_error(),
        "a fresh key never admits its ungranted upload Read/write intent"
    );
    assert_eq!(counts(&o).await, before);
    assert_eq!(
        state(&o, accepted.id).await.plan["import_operation"],
        origin
    );
}

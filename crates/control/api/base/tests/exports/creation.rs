//! The queued archive keeps its exact act and real content sources through native byte delivery.
use super::*;
use async_trait::async_trait;
use orch_api_contract::exports::{ExportCreationReceipt, ExportCreationResponse, NodeExportCreate};
use orch_blob_contract::{
    range::ByteRange,
    store::{BlobError, BlobRead, BlobStore, ByteStream, Stored},
};
use orch_types::Hash;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Notify;

const CREATE: &str = "NodeExportCreate";
fn sources(o: &Office) -> Vec<Value> {
    vec![
        rule(CREATE, "org", o.org(), "self", &["org"], &["read"]),
        rule(
            CREATE,
            "document",
            o.document.base.id,
            "self",
            &["document"],
            &["read"],
        ),
    ]
}
async fn ask(o: &Office, secret: &str, request: &ExportRequest) -> RawReply {
    let call = o.s.rig.world.app.call(NodeExportCreate, &[&o.org()]);
    call.token(secret).body(request).send_bytes().await
}
fn accepted(reply: RawReply) -> ExportCreationReceipt {
    assert_eq!(
        reply.status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: Value = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(
        value.as_object().unwrap().len(),
        3,
        "a receipt exposes only its kind and the two actual identities"
    );
    match serde_json::from_value::<ExportCreationResponse>(value).unwrap() {
        ExportCreationResponse::Receipt(value) => value,
        ExportCreationResponse::Full(_) => {
            panic!("source content Read does not lend private job creator Read")
        }
    }
}
async fn counts(o: &Office) -> (i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM exports),(SELECT count(*) FROM jobs WHERE kind='export')",
    )
    .fetch_one(o.s.rig.db.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn archive_creation_uses_its_exact_root_and_selected_native_content_before_any_queue_write() {
    let o = Office::new().await;
    let before = counts(&o).await;
    for rules in [
        vec![rule(
            "NodeExportPreflight",
            "org",
            o.org(),
            "subtree",
            &["org", "document"],
            &["read"],
        )],
        vec![rule(
            CREATE,
            "org",
            o.org(),
            "subtree",
            &["org", "document"],
            &["manage"],
        )],
        vec![rule(
            CREATE,
            "project",
            o.org(),
            "subtree",
            &["org", "document"],
            &["read"],
        )],
        vec![rule(
            CREATE,
            "document",
            o.document.base.id,
            "self",
            &["document"],
            &["read"],
        )],
    ] {
        let denied = key(&o, &o.s.ada, rules).await;
        assert_eq!(
            ask(&o, &denied.secret, &everything()).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            counts(&o).await,
            before,
            "a refused act never leaves an export or a queued job"
        );
    }
    let writer = key(&o, &o.s.ada, sources(&o)).await;
    let receipt = accepted(ask(&o, &writer.secret, &everything()).await);
    let job = orch_db::queries::jobs::job(&mut *o.s.rig.world.conn().await, receipt.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.plan["export"], json!(receipt.export_id));
    assert_eq!(job.plan["export_operation"]["credential"]["kind"], "token");
    assert_eq!(
        job.plan["export_operation"]["credential"]["id"],
        json!(writer.id)
    );
    assert_eq!(job.plan["export_operation_required"], true);
    let ended = o.ended(&o.s.ada, receipt.id).await;
    assert_eq!(ended.state, JobState::Succeeded, "{:?}", ended.error);
    let export = o.read(&o.s.ada, receipt.export_id).await.ok();
    let counts = export.counts.unwrap();
    assert_eq!(
        (counts.conversations, counts.chats, counts.documents),
        (0, 0, 1)
    );
    let content = o.contents(&o.s.ada, &export).await;
    assert!(has(&content, "Handbook.md"));
    assert!(
        !content
            .keys()
            .any(|path| path.contains("Fix login") || path.ends_with("chat.md")),
        "unbound native sources contribute no archive bytes"
    );
}

#[tokio::test]
async fn queued_job_metadata_needs_this_operations_actual_creator_read() {
    let o = Office::new().await;
    let mut wrong = sources(&o);
    wrong.push(rule(
        "UserGet",
        "account",
        o.s.ada.id(),
        "self",
        &["account"],
        &["read"],
    ));
    let reader = key(&o, &o.s.ada, wrong).await;
    let receipt = accepted(ask(&o, &reader.secret, &everything()).await);
    assert_eq!(
        o.ended(&o.s.ada, receipt.id).await.state,
        JobState::Succeeded
    );
    let mut full = sources(&o);
    full.push(rule(
        CREATE,
        "account",
        o.s.ada.id(),
        "self",
        &["account"],
        &["read"],
    ));
    let reader = key(&o, &o.s.ada, full).await;
    let reply = ask(&o, &reader.secret, &everything()).await;
    assert_eq!(
        reply.status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let answer: ExportCreationResponse = serde_json::from_slice(&reply.raw).unwrap();
    let job = answer
        .into_full()
        .expect("the exact operation may read its actual creator");
    assert_eq!(job.kind, "export");
    assert_eq!(job.created_by.unwrap().id, o.s.ada.id());
    assert_eq!(o.ended(&o.s.ada, job.id).await.state, JobState::Succeeded);
}

struct PausedStore {
    inner: Arc<dyn BlobStore>,
    armed: AtomicBool,
    reached: Notify,
    release: Notify,
}
#[async_trait]
impl BlobStore for PausedStore {
    async fn put(
        &self,
        hash: &Hash,
        size: Option<i64>,
        content: ByteStream,
    ) -> Result<Stored, BlobError> {
        if self.armed.load(Ordering::SeqCst) {
            self.reached.notify_one();
            self.release.notified().await;
        }
        self.inner.put(hash, size, content).await
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
async fn paused_office() -> (Office, Arc<PausedStore>) {
    let slot = Arc::new(std::sync::OnceLock::new());
    let capture = slot.clone();
    let office = Office::with_blob_layer(Box::new(move |inner| {
        let store = Arc::new(PausedStore {
            inner,
            armed: AtomicBool::new(false),
            reached: Notify::new(),
            release: Notify::new(),
        });
        assert!(capture.set(store.clone()).is_ok());
        store
    }))
    .await;
    (office, slot.get().unwrap().clone())
}
async fn reached(o: &Office, store: &PausedStore, job: Id) {
    let wait = store.reached.notified();
    tokio::pin!(wait);
    tokio::time::timeout(rig::PATIENCE,async {
        loop {tokio::select! {
            ()=&mut wait=>break,
            ()=tokio::time::sleep(o.s.rig.engine.config().sweep_every)=>{
                let job=orch_db::queries::jobs::job(&mut *o.s.rig.world.conn().await,job).await.unwrap().unwrap();
                assert!(job.is_open(),"the real archive writer stopped before its supplier wait: {:?}",job.error);
            }
        }}
    }).await.expect("the actual native store must receive the archive");
}
#[tokio::test]
async fn the_native_store_wait_rechecks_exact_scope_expiry_and_actual_document_placement() {
    for mode in ["unchanged", "scope_removed", "expired", "rehome"] {
        let (o, store) = paused_office().await;
        let other = places::make_org(
            &o.s.rig.world,
            &o.s.ada,
            "Another export home",
            "another-export",
        )
        .await
        .base
        .id;
        store.armed.store(true, Ordering::SeqCst);
        let writer = key(&o, &o.s.ada, sources(&o)).await;
        let receipt = accepted(ask(&o, &writer.secret, &everything()).await);
        reached(&o, &store, receipt.id).await;
        assert_eq!(
            o.read(&o.s.ada, receipt.export_id).await.ok().state,
            ExportState::Pending
        );
        match mode {
            "scope_removed" => {
                let scope: KeyScope =
                    serde_json::from_value(json!({"rules":[sources(&o)[0].clone()]})).unwrap();
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
                    .execute(o.s.rig.db.pool())
                    .await
                    .unwrap();
                assert_eq!(moved.rows_affected(), 1);
            }
            _ => {}
        }
        store.release.notify_one();
        let ended = o.ended(&o.s.ada, receipt.id).await;
        assert_eq!(
            ended.state,
            if mode == "unchanged" {
                JobState::Succeeded
            } else {
                JobState::Failed
            },
            "{mode}: {:?}",
            ended.error
        );
        let export = o.read(&o.s.ada, receipt.export_id).await.ok();
        assert_eq!(
            export.state == ExportState::Ready,
            mode == "unchanged",
            "{mode}"
        );
        if mode != "unchanged" {
            assert_eq!(
                o.fetch(&o.s.ada, receipt.export_id).await.status,
                StatusCode::GONE
            );
        }
    }
}
#[tokio::test]
async fn missing_null_and_cross_job_retained_origins_never_choose_an_ordinary_worker_fallback() {
    for mode in ["missing", "null", "cross_job"] {
        let (o, store) = paused_office().await;
        store.armed.store(true, Ordering::SeqCst);
        let writer = key(&o, &o.s.ada, sources(&o)).await;
        let receipt = accepted(ask(&o, &writer.secret, &everything()).await);
        reached(&o, &store, receipt.id).await;
        let mut plan: Value = sqlx::query_scalar("SELECT plan FROM jobs WHERE id=$1")
            .bind(receipt.id)
            .fetch_one(o.s.rig.db.pool())
            .await
            .unwrap();
        match mode {
            "missing" => {
                plan.as_object_mut().unwrap().remove("export_operation");
            }
            "null" => plan["export_operation"] = Value::Null,
            "cross_job" => plan["export_operation"]["job"] = json!(new_id()),
            _ => unreachable!(),
        }
        sqlx::query("UPDATE jobs SET plan=$2 WHERE id=$1")
            .bind(receipt.id)
            .bind(plan)
            .execute(o.s.rig.db.pool())
            .await
            .unwrap();
        store.release.notify_one();
        let ended = o.ended(&o.s.ada, receipt.id).await;
        assert_eq!(ended.state, JobState::Failed, "{mode}: {:?}", ended.error);
        assert_ne!(
            o.read(&o.s.ada, receipt.export_id).await.ok().state,
            ExportState::Ready
        );
    }
}

#[tokio::test]
async fn the_real_queue_serialization_wait_checks_expiry_before_any_export_or_job_insert() {
    for expire in [false, true] {
        let o = Office::new().await;
        let writer = key(&o, &o.s.ada, sources(&o)).await;
        let before = counts(&o).await;
        let mut blocker = o.s.rig.db.pool().begin().await.unwrap();
        orch_db::queries::exports::lock_person(&mut blocker, o.s.ada.id())
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = o.s.rig.world.app.clone();
        let root = o.org();
        let token = writer.secret.clone();
        let future = tokio::spawn(async move {
            let call = app
                .call(NodeExportCreate, &[&root])
                .token(&token)
                .body(&everything());
            call.send_bytes().await
        });
        tokio::time::timeout(rig::PATIENCE,async {
            loop {
                assert!(!future.is_finished());
                let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%pg_advisory_xact_lock%exports:%')")
                    .bind(pid).fetch_one(o.s.rig.db.pool()).await.unwrap();
                if waiting {break;} tokio::task::yield_now().await;
            }
        }).await.expect("the actual native per-person archive serialization");
        if expire {
            o.s.rig.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(rig::PATIENCE, future)
            .await
            .unwrap()
            .unwrap();
        if expire {
            assert!(
                reply.status == StatusCode::UNAUTHORIZED || reply.status == StatusCode::FORBIDDEN
            );
            assert_eq!(counts(&o).await, before);
        } else {
            let receipt = accepted(reply);
            assert_eq!(
                o.ended(&o.s.ada, receipt.id).await.state,
                JobState::Succeeded
            );
        }
    }
}
#[tokio::test]
async fn live_document_labels_need_the_export_operations_own_native_reference_sources() {
    for allowed in [false, true] {
        let o = Office::new().await;
        let ref_content = json!([{"type":"ref","kind":"node","id":o.topic()}]);
        sqlx::query(
            "UPDATE document_blocks SET content=$2 WHERE document_id=$1 AND removed_at IS NULL",
        )
        .bind(o.document.base.id)
        .bind(ref_content)
        .execute(o.s.rig.db.pool())
        .await
        .unwrap();
        let mut rules = sources(&o);
        rules.push(rule(
            if allowed {
                CREATE
            } else {
                "NodeExportPreflight"
            },
            "topic",
            o.topic(),
            "self",
            &["topic"],
            &["read"],
        ));
        let writer = key(&o, &o.s.ada, rules).await;
        let receipt = accepted(ask(&o, &writer.secret, &everything()).await);
        let job = o.ended(&o.s.ada, receipt.id).await;
        assert_eq!(
            job.state,
            if allowed {
                JobState::Succeeded
            } else {
                JobState::Failed
            },
            "{:?}",
            job.error
        );
        let export = o.read(&o.s.ada, receipt.export_id).await.ok();
        assert_eq!(export.state == ExportState::Ready, allowed);
    }
}

async fn retry(o: &Office, secret: &str, act: &str, body: &ExportRequest) -> RawReply {
    let node = o.org();
    let call = o.s.rig.world.app.call(NodeExportCreate, &[&node]);
    call.token(secret)
        .header("Idempotency-Key", act)
        .body(body)
        .send_bytes()
        .await
}
#[tokio::test]
async fn creation_replay_reads_only_the_current_job_without_recopying_the_archive() {
    let o = Office::new().await;
    let act = new_id().to_string();
    let body = everything();
    let writer = key(&o, &o.s.ada, sources(&o)).await;
    let receipt = accepted(retry(&o, &writer.secret, &act, &body).await);
    assert_eq!(
        o.ended(&o.s.ada, receipt.id).await.state,
        JobState::Succeeded
    );
    let before = counts(&o).await;
    let original = orch_db::queries::jobs::job(&mut *o.s.rig.world.conn().await, receipt.id)
        .await
        .unwrap()
        .unwrap()
        .plan["export_operation"]
        .clone();
    let reader = key(
        &o,
        &o.s.ada,
        vec![rule(
            CREATE,
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
    let current: ExportCreationResponse = serde_json::from_slice(&response.raw).unwrap();
    let current = current.into_full().unwrap();
    assert_eq!(current.id, receipt.id);
    assert_eq!(
        current.state,
        JobState::Succeeded,
        "the current native job is rendered, not the first queued body"
    );
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
        accepted(retry(&o, &wrong.secret, &act, &body).await),
        receipt
    );
    assert_eq!(
        retry(&o, &reader.secret, &new_id().to_string(), &body)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(counts(&o).await, before);
    assert_eq!(
        orch_db::queries::jobs::job(&mut *o.s.rig.world.conn().await, receipt.id)
            .await
            .unwrap()
            .unwrap()
            .plan["export_operation"],
        original
    );
}

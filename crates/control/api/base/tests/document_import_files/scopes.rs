//! Actual custody, per-entry transactions and completed report Read share one import operation.
use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::{
    documents::{
        DocumentFileCreate, DocumentFileCreateQuery, DocumentFileDelete, NewDocumentAccess,
    },
    threads::FILENAME_HEADER,
};
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_blob_contract::{
    range::ByteRange,
    store::{BlobError, BlobRead, BlobStore, ByteStream, Stored},
};
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Json, new_id};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::Notify;

const OP: &str = "NodeDocumentImport";
const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
pub(super) struct Key {
    pub(super) id: Id,
    pub(super) secret: String,
}
pub(super) fn rule(
    operation: &str,
    kind: &str,
    id: Id,
    reach: &str,
    resources: &[&str],
    actions: &[&str],
) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"resources":resources,"allow":[{"operation":operation,"actions":actions}]})
}
fn scope(rules: Vec<Json>) -> Json {
    let value: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(value.validate().is_empty());
    json!(value)
}
pub(super) async fn key(desk: &Desk, user: Id, rules: Vec<Json>) -> Key {
    let key = Key {
        id: new_id(),
        secret: new_token(API_TOKEN_PREFIX),
    };
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: key.id,
            user_id: user,
            name: "Native partial document import",
            token_hash: &token_hash(&key.secret),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: Some(desk.world.now() + chrono::Duration::hours(1)),
        },
        Some(&scope(rules)),
    )
    .await
    .unwrap();
    key
}
pub(super) fn destination(desk: &Desk, read: bool) -> Json {
    rule(
        OP,
        "topic",
        desk.topic,
        if read { "subtree" } else { "self" },
        &["document"],
        if read { &["write", "read"] } else { &["write"] },
    )
}
pub(super) fn retained(desk: &Desk) -> Json {
    rule(OP, "account", desk.ada.id(), "self", &["upload"], &["read"])
}
fn body(files: Vec<(Hash, &str, Format)>, client: &str) -> DocumentImportRequest {
    DocumentImportRequest {
        client_id: client.into(),
        files: files
            .into_iter()
            .map(|(blob, name, format)| DocumentImportRequestFile {
                blob,
                name: name.into(),
                format,
            })
            .collect(),
    }
}
async fn call(desk: &Desk, secret: &str, body: &DocumentImportRequest) -> RawReply {
    let request = desk
        .world
        .app
        .call(NodeDocumentImport, &[&desk.topic])
        .token(secret)
        .body(body);
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
async fn state(desk: &Desk) -> Json {
    sqlx::query_scalar("SELECT jsonb_build_object('documents',(SELECT jsonb_agg(to_jsonb(d) ORDER BY id) FROM documents d WHERE node_id=$1),'files',(SELECT jsonb_agg(to_jsonb(f) ORDER BY f.id) FROM document_files f JOIN documents d ON d.id=f.document_id WHERE d.node_id=$1),'versions',(SELECT jsonb_agg(to_jsonb(v) ORDER BY v.document_id,v.version) FROM document_versions v JOIN documents d ON d.id=v.document_id WHERE d.node_id=$1),'created',(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM audit_log a WHERE action='document.created' AND node_id=$1))").bind(desk.topic).fetch_one(&desk.world.db.pool).await.unwrap()
}
pub(super) async fn wait(
    desk: &Desk,
    pid: i32,
    query: &str,
    task: &tokio::task::JoinHandle<RawReply>,
) {
    tokio::time::timeout(WAIT,async{loop{assert!(!task.is_finished(),"the actual native query must wait");let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)").bind(pid).bind(query).fetch_one(&desk.world.db.pool).await.unwrap();if blocked {break;}tokio::task::yield_now().await;}}).await.expect("observe the literal native blocker PID");
}
struct Probe {
    inner: Arc<dyn BlobStore>,
    hash: Hash,
    pause: bool,
    used: AtomicBool,
    gets: AtomicUsize,
    puts: AtomicUsize,
    reached: Notify,
    release: Notify,
}
#[async_trait::async_trait]
impl BlobStore for Probe {
    async fn put(
        &self,
        hash: &Hash,
        size: Option<i64>,
        body: ByteStream,
    ) -> Result<Stored, BlobError> {
        self.puts.fetch_add(1, Ordering::SeqCst);
        self.inner.put(hash, size, body).await
    }
    async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        if *hash == self.hash {
            self.gets.fetch_add(1, Ordering::SeqCst);
        }
        let result = self.inner.get(hash, range).await;
        if *hash == self.hash && self.pause && !self.used.swap(true, Ordering::SeqCst) {
            self.reached.notify_one();
            self.release.notified().await;
        }
        result
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
fn probe(desk: &mut Desk, hash: Hash, pause: bool) -> Arc<Probe> {
    use orch_api_core::{registry::Registry, state::AppState, testing::TestApp};
    let old = desk.world.app.app.clone();
    let probe = Arc::new(Probe {
        inner: old.blobs.store(),
        hash,
        pause,
        used: AtomicBool::new(false),
        gets: AtomicUsize::new(0),
        puts: AtomicUsize::new(0),
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
pub(super) async fn source_document(desk: &Desk, bytes: &[u8]) -> (Id, Id) {
    let mut body = desk::body("Real import custody");
    body.access = Some(NewDocumentAccess::Private);
    let document = desk.make_body(&desk.ada, body).await.ok().base.id;
    let request = desk
        .world
        .app
        .call(DocumentFileCreate, &[&document])
        .token(&desk.ada.token)
        .query(&DocumentFileCreateQuery::default())
        .header(FILENAME_HEADER, "source.md")
        .bytes(bytes);
    let file = request.send().await.ok().into_full().unwrap().id;
    (document, file)
}

#[tokio::test]
async fn write_only_partial_import_retains_native_ids_order_failures_and_durable_replay_without_get_read()
 {
    let mut desk = Desk::new().await;
    let blob = upload(
        &desk,
        &desk.ada,
        "source.md",
        b"# New import\n\nOnly supplied words.",
    )
    .await;
    let token = key(
        &desk,
        desk.ada.id(),
        vec![
            destination(&desk, false),
            retained(&desk),
            rule(
                "DocumentGet.read",
                "topic",
                desk.topic,
                "subtree",
                &["document"],
                &["read"],
            ),
        ],
    )
    .await;
    let probe = probe(&mut desk, blob, false);
    let request = body(
        vec![
            (blob, "source.md", Format::Markdown),
            (blob, "source.md", Format::Markdown),
        ],
        "scoped-import-replay",
    );
    let value = accepted(call(&desk, &token.secret, &request).await);
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["entries"].as_array().unwrap().len(), 2);
    let made: Id = serde_json::from_value(value["entries"][0]["document_id"].clone()).unwrap();
    assert!(value["entries"][0]["error"].is_null());
    assert!(value["entries"][1]["document_id"].is_null());
    assert!(value["entries"][1]["error"].is_string());
    assert!(value["entries"][0].get("document").is_none());
    assert!(value["entries"][0].get("warnings").is_none());
    let saved = state(&desk).await;
    let again = accepted(call(&desk, &token.secret, &request).await);
    assert_eq!(again, value);
    assert_eq!(state(&desk).await, saved);
    let ordinary = desk
        .world
        .app
        .call(DocumentGet, &[&made])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(ordinary.base.title, "New import");
    assert_eq!(probe.puts.load(Ordering::SeqCst), 0);
    assert_eq!(probe.gets.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn full_import_keeps_archive_links_companion_files_and_same_operation_projections() {
    let mut desk = Desk::new().await;
    let archive = zip(&[
        (
            "Home.md",
            b"# Home\n\n[Guide](Guide.md)\n\n![own](shot.png)\n",
        ),
        ("Guide.md", b"# Guide\n\n[Home](Home.md)\n"),
        ("shot.png", PICTURE),
    ]);
    let blob = upload(&desk, &desk.ada, "source.zip", &archive).await;
    let token = key(
        &desk,
        desk.ada.id(),
        vec![
            destination(&desk, true),
            retained(&desk),
            rule(
                OP,
                "topic",
                desk.topic,
                "subtree",
                &["topic", "project", "org"],
                &["read"],
            ),
        ],
    )
    .await;
    let probe = probe(&mut desk, blob, false);
    let request = body(
        vec![(blob, "source.zip", Format::Archive)],
        "scoped-archive",
    );
    let full: DocumentImportResult =
        serde_json::from_value(accepted(call(&desk, &token.secret, &request).await)).unwrap();
    assert_eq!(full.entries.len(), 2);
    assert!(full.entries.iter().all(|entry| entry.error.is_none()));
    let home = full
        .entries
        .iter()
        .find(|entry| entry.name == "Home.md")
        .unwrap()
        .document
        .as_ref()
        .unwrap();
    let guide = full
        .entries
        .iter()
        .find(|entry| entry.name == "Guide.md")
        .unwrap()
        .document
        .as_ref()
        .unwrap();
    assert!(
        chips(&desk, home.id)
            .await
            .iter()
            .any(|(_, id)| *id == guide.id)
    );
    assert!(
        chips(&desk, guide.id)
            .await
            .iter()
            .any(|(_, id)| *id == home.id)
    );
    let files: i64 = sqlx::query_scalar("SELECT count(*) FROM document_files WHERE document_id=$1")
        .bind(home.id)
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap();
    assert_eq!(files, 1);
    assert_eq!(probe.gets.load(Ordering::SeqCst), 1);
    assert_eq!(probe.puts.load(Ordering::SeqCst), 1);
    let saved = state(&desk).await;
    let replay: DocumentImportResult =
        serde_json::from_value(accepted(call(&desk, &token.secret, &request).await)).unwrap();
    assert_eq!(replay, full);
    assert_eq!(state(&desk).await, saved);
    assert_eq!(probe.gets.load(Ordering::SeqCst), 2);
    assert_eq!(
        probe.puts.load(Ordering::SeqCst),
        1,
        "native replay never finishes or supplies embedded files again"
    );
}

#[tokio::test]
async fn destination_business_kind_operation_and_native_door_precede_any_source_bytes() {
    let mut desk = Desk::new().await;
    let blob = upload(
        &desk,
        &desk.ada,
        "source.md",
        b"# Protected native import\n",
    )
    .await;
    let probe = probe(&mut desk, blob, false);
    let before = state(&desk).await;
    for wrong in [
        rule(
            OP,
            "topic",
            desk.topic,
            "self",
            &["environment"],
            &["write"],
        ),
        rule(
            "NodeDocumentCreate",
            "topic",
            desk.topic,
            "self",
            &["document"],
            &["write"],
        ),
        rule(OP, "topic", new_id(), "self", &["document"], &["write"]),
    ] {
        let token = key(&desk, desk.ada.id(), vec![wrong, retained(&desk)]).await;
        let result = call(
            &desk,
            &token.secret,
            &body(
                vec![(blob, "source.md", Format::Markdown)],
                "wrong-native-door",
            ),
        )
        .await;
        assert_eq!(result.status, StatusCode::FORBIDDEN);
        assert_eq!(state(&desk).await, before);
        assert_eq!(probe.gets.load(Ordering::SeqCst), 0);
    }
    let stranger = desk.stranger("import-stranger").await;
    let token = key(
        &desk,
        stranger.id(),
        vec![
            destination(&desk, false),
            rule(OP, "account", stranger.id(), "self", &["upload"], &["read"]),
        ],
    )
    .await;
    assert!(matches!(
        call(
            &desk,
            &token.secret,
            &body(
                vec![(blob, "source.md", Format::Markdown)],
                "stranger-native-door"
            )
        )
        .await
        .status,
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
    ));
    assert_eq!(probe.gets.load(Ordering::SeqCst), 0);
    assert_eq!(state(&desk).await, before);
}

#[tokio::test]
async fn initial_store_wait_rechecks_raw_key_and_the_original_custodian_without_alternative_upload_adoption()
 {
    for change in ["none", "expiry", "scope", "custody", "placement"] {
        let mut desk = Desk::new().await;
        let moved_to = if change == "placement" {
            Some(desk.another_topic("Moved source").await)
        } else {
            None
        };
        let bytes = b"# Fixed original source\n\nA native source witness.\n";
        let (owner, file) = source_document(&desk, bytes).await;
        let blob = Hash::of(bytes);
        // File creation retains a personal copy as well as its native Document pin. Remove only
        // that real personal reference to force the initial selected source to be the Document.
        sqlx::query("DELETE FROM completed_uploads WHERE user_id=$1 AND hash=$2")
            .bind(desk.ada.id())
            .bind(blob.as_bytes().as_slice())
            .execute(&desk.world.db.pool)
            .await
            .unwrap();
        assert!(
            !orch_db::queries::uploads::owns(&mut *desk.world.conn().await, desk.ada.id(), &blob)
                .await
                .unwrap()
        );
        let rules = vec![
            destination(&desk, false),
            rule(OP, "document", owner, "self", &["document"], &["read"]),
            retained(&desk),
        ];
        let token = key(&desk, desk.ada.id(), rules).await;
        let probe = probe(&mut desk, blob, true);
        let app = desk.world.app.clone();
        let secret = token.secret.clone();
        let node = desk.topic;
        let request_body = body(
            vec![(blob, "source.md", Format::Markdown)],
            "fixed-source-import",
        );
        let task = tokio::spawn(async move {
            let request = app
                .call(NodeDocumentImport, &[&node])
                .token(&secret)
                .body(&request_body);
            request.send_bytes().await
        });
        tokio::time::timeout(WAIT, probe.reached.notified())
            .await
            .unwrap();
        if change == "expiry" {
            desk.world.advance(chrono::Duration::hours(2));
        } else if change == "scope" {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(token.id)
                .bind(scope(vec![destination(&desk, false)]))
                .execute(&desk.world.db.pool)
                .await
                .unwrap();
        } else if change == "custody" {
            let request = desk
                .world
                .app
                .call(DocumentFileDelete, &[&owner, &file])
                .token(&desk.ada.token);
            request.send().await.ok();
            let alternative = upload(&desk, &desk.ada, "retained-now.md", bytes).await;
            assert_eq!(alternative, blob);
        } else if let Some(node) = moved_to {
            sqlx::query("UPDATE documents SET node_id=$2 WHERE id=$1")
                .bind(owner)
                .bind(node)
                .execute(&desk.world.db.pool)
                .await
                .unwrap();
        }
        let before = state(&desk).await;
        let gets = probe.gets.load(Ordering::SeqCst);
        probe.release.notify_one();
        let result = tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
        if change == "none" {
            let value = accepted(result);
            assert!(value["entries"][0]["document_id"].is_string());
        } else {
            assert!(matches!(
                result.status,
                StatusCode::UNAUTHORIZED
                    | StatusCode::FORBIDDEN
                    | StatusCode::NOT_FOUND
                    | StatusCode::OK
            ));
            if result.status == StatusCode::OK {
                let value: Json = serde_json::from_slice(&result.raw).unwrap();
                assert!(value["entries"][0]["document_id"].is_null());
                assert!(value["entries"][0]["document"].is_null());
            }
            assert_eq!(
                state(&desk).await,
                before,
                "{change}: source loss leaves no new custody/document"
            );
        }
        assert_eq!(
            probe.gets.load(Ordering::SeqCst),
            gets,
            "the initial source is not reopened"
        );
    }
}

#[tokio::test]
async fn the_native_preparation_keeps_both_real_byte_sources_when_only_one_entry_decodes() {
    let mut desk = Desk::new().await;
    let good_bytes = b"# Accepted text\n\nThe first entry remains independent.\n";
    let bad_bytes = b"This is not a Word archive.";
    let good = Hash::of(good_bytes);
    let bad = Hash::of(bad_bytes);
    let (good_owner, _) = source_document(&desk, good_bytes).await;
    let (bad_owner, _) = source_document(&desk, bad_bytes).await;
    for hash in [good, bad] {
        sqlx::query("DELETE FROM completed_uploads WHERE user_id=$1 AND hash=$2")
            .bind(desk.ada.id())
            .bind(hash.as_bytes().as_slice())
            .execute(&desk.world.db.pool)
            .await
            .unwrap();
    }
    let token = key(
        &desk,
        desk.ada.id(),
        vec![
            destination(&desk, false),
            rule(OP, "document", good_owner, "self", &["document"], &["read"]),
            rule(OP, "document", bad_owner, "self", &["document"], &["read"]),
        ],
    )
    .await;
    let probe = probe(&mut desk, good, false);
    let result = accepted(
        call(
            &desk,
            &token.secret,
            &body(
                vec![
                    (good, "good.md", Format::Markdown),
                    (bad, "bad.docx", Format::Docx),
                ],
                "partial-native-preparation",
            ),
        )
        .await,
    );
    assert_eq!(result["projection"], "receipt");
    let document: Id = serde_json::from_value(result["entries"][0]["document_id"].clone()).unwrap();
    assert!(result["entries"][0]["error"].is_null());
    assert!(result["entries"][1]["document_id"].is_null());
    assert!(result["entries"][1]["error"].is_string());
    let stored = desk
        .world
        .app
        .call(DocumentGet, &[&document])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(stored.base.title, "Accepted text");
    assert_eq!(probe.gets.load(Ordering::SeqCst), 1);
    assert_eq!(probe.puts.load(Ordering::SeqCst), 0);
    let created: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='document.created' AND node_id=$1",
    )
    .bind(desk.topic)
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        created, 3,
        "two native source documents and the accepted entry"
    );
}

#[tokio::test]
async fn a_later_real_file_insert_wait_preserves_the_first_entry_and_rolls_back_only_the_expired_second()
 {
    for expired in [false, true] {
        let mut desk = Desk::new().await;
        let first = upload(
            &desk,
            &desk.ada,
            "first.md",
            b"# First accepted\n\nFirst body.\n",
        )
        .await;
        let picture = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, PICTURE);
        let second_text =
            format!("# Second accepted\n\n![inline](data:image/png;base64,{picture})\n");
        let second = upload(&desk, &desk.ada, "second.md", second_text.as_bytes()).await;
        let token = key(
            &desk,
            desk.ada.id(),
            vec![destination(&desk, false), retained(&desk)],
        )
        .await;
        let probe = probe(&mut desk, second, false);
        let mut blocker = desk.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE document_files IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = desk.world.app.clone();
        let secret = token.secret.clone();
        let node = desk.topic;
        let request_body = body(
            vec![
                (first, "first.md", Format::Markdown),
                (second, "second.md", Format::Markdown),
            ],
            "partial-custody-wait",
        );
        let task = tokio::spawn(async move {
            let request = app
                .call(NodeDocumentImport, &[&node])
                .token(&secret)
                .body(&request_body);
            request.send_bytes().await
        });
        wait(&desk, pid, "%INSERT INTO document_files%", &task).await;
        let first_id: Id = sqlx::query_scalar(
            "SELECT id FROM documents WHERE node_id=$1 AND title='First accepted'",
        )
        .bind(node)
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap();
        let before = state(&desk).await;
        if expired {
            desk.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let value = accepted(tokio::time::timeout(WAIT, task).await.unwrap().unwrap());
        assert_eq!(value["entries"][0]["document_id"], json!(first_id));
        if expired {
            assert!(value["entries"][1]["document_id"].is_null());
            assert!(value["entries"][1]["error"].is_string());
            assert_eq!(
                state(&desk).await,
                before,
                "the second native header, files, pins, history and audit roll back together"
            );
        } else {
            let id: Id =
                serde_json::from_value(value["entries"][1]["document_id"].clone()).unwrap();
            assert_ne!(id, first_id);
        }
        assert_eq!(probe.gets.load(Ordering::SeqCst), 1);
        assert_eq!(probe.puts.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn completed_partial_report_uses_current_result_read_without_reapplying_destination_or_source_input_authority()
 {
    for change in ["none", "input", "read", "expiry", "placement"] {
        let mut desk = Desk::new().await;
        let blob = upload(
            &desk,
            &desk.ada,
            "source.md",
            b"# Completed report\n\nA stored original.\n",
        )
        .await;
        let token = key(
            &desk,
            desk.ada.id(),
            vec![destination(&desk, true), retained(&desk)],
        )
        .await;
        let probe = probe(&mut desk, blob, false);
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
        let node = desk.topic;
        let request_body = body(
            vec![(blob, "source.md", Format::Markdown)],
            "completed-report-cut",
        );
        let task = tokio::spawn(async move {
            let request = app
                .call(NodeDocumentImport, &[&node])
                .token(&secret)
                .body(&request_body);
            request.send_bytes().await
        });
        wait(&desk, pid, "%INSERT INTO api_token_activity%", &task).await;
        let id: Id = sqlx::query_scalar(
            "SELECT id FROM documents WHERE node_id=$1 AND title='Completed report'",
        )
        .bind(node)
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap();
        if change == "input" {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(token.id)
                .bind(scope(vec![rule(
                    OP,
                    "document",
                    id,
                    "self",
                    &["document"],
                    &["read"],
                )]))
                .execute(&desk.world.db.pool)
                .await
                .unwrap();
        } else if change == "read" {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(token.id)
                .bind(scope(vec![destination(&desk, false), retained(&desk)]))
                .execute(&desk.world.db.pool)
                .await
                .unwrap();
        } else if change == "expiry" {
            desk.world.advance(chrono::Duration::hours(2));
        } else if change == "placement" {
            sqlx::query("UPDATE documents SET deleted_at=$2 WHERE id=$1")
                .bind(id)
                .bind(desk.world.now())
                .execute(&desk.world.db.pool)
                .await
                .unwrap();
        }
        let committed = state(&desk).await;
        blocker.commit().await.unwrap();
        let value = accepted(tokio::time::timeout(WAIT, task).await.unwrap().unwrap());
        if change == "none" || change == "input" {
            assert_eq!(value["entries"][0]["document"]["id"], json!(id));
            assert!(value.get("projection").is_none());
        } else {
            assert_eq!(
                value,
                json!({"projection":"receipt","entries":[{"name":"source.md","document_id":id,"error":null}]})
            );
        }
        assert_eq!(state(&desk).await, committed);
        assert_eq!(probe.gets.load(Ordering::SeqCst), 1);
        assert_eq!(probe.puts.load(Ordering::SeqCst), 0);
    }
}

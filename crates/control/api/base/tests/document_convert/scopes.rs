//! Conversion binds actual actor/context sources before one native CPU/byte stage and real custody.

use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
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
use orch_types::{Hash, Id, Json, new_id};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::Notify;

const SUPPLIED: &str = "DocumentConvert.supplied";
const DOCUMENT: &str = "DocumentConvert.document";
const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
struct Key {
    id: Id,
    secret: String,
}
fn rule(op: &str, kind: &str, id: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":op,"actions":actions}]})
}
fn scope(rules: Vec<Json>) -> Json {
    let value: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(value.validate().is_empty());
    json!(value)
}
async fn key(desk: &Desk, who: Id, rules: Vec<Json>) -> Key {
    let key = Key {
        id: new_id(),
        secret: new_token(API_TOKEN_PREFIX),
    };
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: key.id,
            user_id: who,
            name: "Native document conversion",
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
async fn document(desk: &Desk) -> Id {
    let mut body = desk::body("Native conversion context");
    body.access = Some(NewDocumentAccess::Private);
    desk.make_body(&desk.ada, body).await.ok().base.id
}
fn body(document: Option<Id>, text: &str) -> DocumentConvertBody {
    DocumentConvertBody {
        format: DocumentConvertFormat::Markdown,
        text: text.into(),
        document_id: document,
    }
}
fn image(document: Id) -> DocumentConvertBody {
    body(
        Some(document),
        &format!(
            "Before

![new picture](data:image/png;base64,{PICTURE})
"
        ),
    )
}
async fn call(desk: &Desk, secret: &str, body: &DocumentConvertBody) -> RawReply {
    let request = desk
        .world
        .app
        .call(DocumentConvert, &[])
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
async fn file(desk: &Desk, doc: Id, name: &str, bytes: &[u8]) -> Id {
    let request = desk
        .world
        .app
        .call(DocumentFileCreate, &[&doc])
        .token(&desk.ada.token)
        .query(&DocumentFileCreateQuery::default())
        .header(FILENAME_HEADER, name)
        .bytes(bytes);
    request.send().await.ok().into_full().unwrap().id
}
async fn state(desk: &Desk, doc: Id) -> Json {
    sqlx::query_scalar("SELECT jsonb_build_object('document',(SELECT to_jsonb(d) FROM documents d WHERE id=$1),'files',(SELECT jsonb_agg(to_jsonb(f) ORDER BY id) FROM document_files f WHERE document_id=$1),'versions',(SELECT jsonb_agg(to_jsonb(v) ORDER BY version) FROM document_versions v WHERE document_id=$1),'blobs',(SELECT jsonb_agg(to_jsonb(b) ORDER BY hash) FROM blobs b))").bind(doc).fetch_one(&desk.world.db.pool).await.unwrap()
}
async fn wait(desk: &Desk, pid: i32, pattern: &str, task: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(WAIT,async{loop{assert!(!task.is_finished(),"actual native wait not reached");let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)").bind(pid).bind(pattern).fetch_one(&desk.world.db.pool).await.unwrap();if blocked{break;}tokio::task::yield_now().await;}}).await.expect("observe the native query blocked by the literal PID");
}
struct Probe {
    inner: Arc<dyn BlobStore>,
    hash: Hash,
    pause: bool,
    used: AtomicBool,
    puts: AtomicUsize,
    gets: AtomicUsize,
    reached: Notify,
    release: Notify,
}
impl Probe {
    async fn reached(&self) {
        tokio::time::timeout(WAIT, self.reached.notified())
            .await
            .expect("the actual native byte store put must wait");
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
        if *hash == self.hash && self.pause && !self.used.swap(true, Ordering::SeqCst) {
            self.reached.notify_one();
            self.release.notified().await;
        }
        stored
    }
    async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        if *hash == self.hash {
            self.gets.fetch_add(1, Ordering::SeqCst);
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
fn probe(desk: &mut Desk, pause: bool) -> Arc<Probe> {
    use orch_api_core::{registry::Registry, state::AppState, testing::TestApp};
    let old = desk.world.app.app.clone();
    let probe = Arc::new(Probe {
        inner: old.blobs.store(),
        hash: Hash::of(&STANDARD.decode(PICTURE).unwrap()),
        pause,
        used: AtomicBool::new(false),
        puts: AtomicUsize::new(0),
        gets: AtomicUsize::new(0),
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
async fn supplied_conversion_uses_the_actual_account_without_document_or_membership_ancestry() {
    let mut desk = Desk::new().await;
    let probe = probe(&mut desk, false);
    let valid = key(
        &desk,
        desk.ada.id(),
        vec![rule(SUPPLIED, "account", desk.ada.id(), &["write"])],
    )
    .await;
    for format in [
        DocumentConvertFormat::Markdown,
        DocumentConvertFormat::Html,
        DocumentConvertFormat::Text,
    ] {
        let request = DocumentConvertBody {
            format,
            text: "Provided words

More supplied words"
                .into(),
            document_id: None,
        };
        let full: DocumentConvertReply =
            serde_json::from_value(accepted(call(&desk, &valid.secret, &request).await)).unwrap();
        assert!(!full.blocks.is_empty());
        assert!(full.files.is_empty());
    }
    let text = format!("![not retained](data:image/png;base64,{PICTURE})");
    let full: DocumentConvertReply = serde_json::from_value(accepted(
        call(&desk, &valid.secret, &body(None, &text)).await,
    ))
    .unwrap();
    assert!(full.files.is_empty());
    assert!(
        full.warnings
            .iter()
            .any(|warning| warning.code == "reader.pictures_left_out")
    );
    for wrong in [
        rule(SUPPLIED, "org", desk.org, &["write"]),
        rule(SUPPLIED, "account", new_id(), &["write"]),
        rule(DOCUMENT, "account", desk.ada.id(), &["write", "read"]),
    ] {
        let wrong = key(&desk, desk.ada.id(), vec![wrong]).await;
        assert_eq!(
            call(&desk, &wrong.secret, &body(None, "words"))
                .await
                .status,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(probe.puts.load(Ordering::SeqCst), 0);
    assert_eq!(probe.gets.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn contextual_directory_reads_are_same_operation_and_kind_before_any_embedded_supplier() {
    let mut desk = Desk::new().await;
    let doc = document(&desk).await;
    let old = file(
        &desk,
        doc,
        "existing.png",
        &STANDARD.decode(PICTURE).unwrap(),
    )
    .await;
    let probe = probe(&mut desk, false);
    let before = state(&desk, doc).await;
    for rules in [
        vec![
            rule(DOCUMENT, "document", doc, &["write"]),
            rule("DocumentGet.read", "document", doc, &["read"]),
        ],
        vec![rule(DOCUMENT, "document", doc, &["read"])],
        vec![rule(
            DOCUMENT,
            "document_container",
            doc,
            &["write", "read"],
        )],
        vec![rule(SUPPLIED, "account", desk.ada.id(), &["write"])],
    ] {
        let wrong = key(&desk, desk.ada.id(), rules).await;
        assert_eq!(
            call(&desk, &wrong.secret, &image(doc)).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(state(&desk, doc).await, before);
        assert_eq!(probe.puts.load(Ordering::SeqCst), 0);
    }
    let token = key(
        &desk,
        desk.ada.id(),
        vec![rule(DOCUMENT, "document", doc, &["write", "read"])],
    )
    .await;
    for format in [DocumentConvertFormat::Markdown, DocumentConvertFormat::Html] {
        let text = if format == DocumentConvertFormat::Markdown {
            "![known](existing.png)"
        } else {
            r#"<p>Known</p><img src="existing.png" alt="known">"#
        };
        let request = DocumentConvertBody {
            format,
            text: text.into(),
            document_id: Some(doc),
        };
        let full: DocumentConvertReply =
            serde_json::from_value(accepted(call(&desk, &token.secret, &request).await)).unwrap();
        let image = full
            .blocks
            .iter()
            .find_map(|block| match &block.props {
                Some(BlockProps::Image(props)) => Some(props.file_id),
                _ => None,
            })
            .unwrap();
        assert_eq!(image, old);
        assert!(full.files.is_empty());
    }
    let stranger = desk.stranger("converter-stranger").await;
    let unowned = key(
        &desk,
        stranger.id(),
        vec![rule(DOCUMENT, "document", doc, &["write", "read"])],
    )
    .await;
    assert!(matches!(
        call(&desk, &unowned.secret, &image(doc)).await.status,
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
    ));
    assert_eq!(probe.puts.load(Ordering::SeqCst), 0);
    let full: DocumentConvertReply =
        serde_json::from_value(accepted(call(&desk, &token.secret, &image(doc)).await)).unwrap();
    assert_eq!(full.files.len(), 1);
    assert_eq!(probe.puts.load(Ordering::SeqCst), 1);
    assert_eq!(probe.gets.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_real_initial_store_wait_keeps_one_conversion_and_never_substitutes_a_new_directory_witness()
 {
    for change in ["none", "expired", "scope", "witness"] {
        let mut desk = Desk::new().await;
        let doc = document(&desk).await;
        let old = file(&desk, doc, "selected.png", b"original selected source").await;
        let token = key(
            &desk,
            desk.ada.id(),
            vec![rule(DOCUMENT, "document", doc, &["write", "read"])],
        )
        .await;
        let probe = probe(&mut desk, true);
        let text = format!(
            "![selected](selected.png)

![new](data:image/png;base64,{PICTURE})"
        );
        let app = desk.world.app.clone();
        let secret = token.secret.clone();
        let task = tokio::spawn(async move {
            let request = app
                .call(DocumentConvert, &[])
                .token(&secret)
                .body(&body(Some(doc), &text));
            request.send_bytes().await
        });
        probe.reached().await;
        if change == "expired" {
            desk.world.advance(chrono::Duration::hours(2));
        } else if change == "scope" {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(token.id)
                .bind(scope(vec![rule(DOCUMENT, "document", doc, &["read"])]))
                .execute(&desk.world.db.pool)
                .await
                .unwrap();
        } else if change == "witness" {
            let request = desk
                .world
                .app
                .call(DocumentFileDelete, &[&doc, &old])
                .token(&desk.ada.token);
            request.send().await.ok();
            let replacement =
                file(&desk, doc, "selected.png", b"a distinct replacement source").await;
            assert_ne!(replacement, old);
        }
        let before = state(&desk, doc).await;
        probe.release.notify_one();
        let reply = tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
        if change == "none" {
            let full: DocumentConvertReply = serde_json::from_value(accepted(reply)).unwrap();
            assert_eq!(full.files.len(), 1);
            assert!(full.blocks.iter().any(
                |block| matches!(&block.props,Some(BlockProps::Image(props))if props.file_id==old)
            ));
        } else {
            assert!(
                matches!(
                    reply.status,
                    StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::CONFLICT
                ),
                "{change}: {}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(
                state(&desk, doc).await,
                before,
                "{change}: no custody, pin, history or touch after the native wait"
            );
        }
        assert_eq!(
            probe.puts.load(Ordering::SeqCst),
            1,
            "the actual embedded supplier runs once"
        );
    }
}

#[tokio::test]
async fn raw_expiry_after_actual_document_blob_and_file_insert_waits_keeps_native_custody_atomic() {
    for footprint in ["document", "blob", "insert"] {
        for expired in [false, true] {
            let mut desk = Desk::new().await;
            let doc = document(&desk).await;
            let token = key(
                &desk,
                desk.ada.id(),
                vec![rule(DOCUMENT, "document", doc, &["write", "read"])],
            )
            .await;
            if footprint == "blob" {
                file(
                    &desk,
                    doc,
                    "baseline.png",
                    &STANDARD.decode(PICTURE).unwrap(),
                )
                .await;
            }
            let probe = probe(&mut desk, false);
            let before = state(&desk, doc).await;
            let mut blocker = desk.world.db.pool.begin().await.unwrap();
            let pattern = match footprint {
                "document" => {
                    sqlx::query("SELECT id FROM documents WHERE id=$1 FOR UPDATE")
                        .bind(doc)
                        .fetch_one(&mut *blocker)
                        .await
                        .unwrap();
                    "%FROM documents%FOR UPDATE%"
                }
                "blob" => {
                    orch_db::queries::uploads::lock_blobs(&mut blocker, &[probe.hash])
                        .await
                        .unwrap();
                    "%FROM blobs%FOR UPDATE%"
                }
                _ => {
                    sqlx::query("LOCK TABLE document_files IN SHARE MODE")
                        .execute(&mut *blocker)
                        .await
                        .unwrap();
                    "%INSERT INTO document_files%"
                }
            };
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let app = desk.world.app.clone();
            let secret = token.secret.clone();
            let task = tokio::spawn(async move {
                let request = app
                    .call(DocumentConvert, &[])
                    .token(&secret)
                    .body(&image(doc));
                request.send_bytes().await
            });
            wait(&desk, pid, pattern, &task).await;
            if expired {
                desk.world.advance(chrono::Duration::hours(2));
            }
            blocker.commit().await.unwrap();
            let reply = tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
            if expired {
                assert!(matches!(
                    reply.status,
                    StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
                ));
                assert_eq!(state(&desk, doc).await, before);
            } else {
                let full: DocumentConvertReply = serde_json::from_value(accepted(reply)).unwrap();
                assert_eq!(full.files.len(), 1);
            }
            assert_eq!(probe.puts.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn completed_conversion_rechecks_current_read_and_exact_file_association_without_parsing_or_storing_again()
 {
    for change in ["none", "input_write", "result_read", "expired", "file"] {
        let mut desk = Desk::new().await;
        let doc = document(&desk).await;
        let token = key(
            &desk,
            desk.ada.id(),
            vec![rule(DOCUMENT, "document", doc, &["write", "read"])],
        )
        .await;
        let probe = probe(&mut desk, false);
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
        let task = tokio::spawn(async move {
            let request = app
                .call(DocumentConvert, &[])
                .token(&secret)
                .body(&image(doc));
            request.send_bytes().await
        });
        wait(&desk, pid, "%INSERT INTO api_token_activity%", &task).await;
        let id: Id = sqlx::query_scalar("SELECT id FROM document_files WHERE document_id=$1")
            .bind(doc)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
        if change == "input_write" || change == "result_read" {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(token.id)
                .bind(scope(vec![rule(
                    DOCUMENT,
                    "document",
                    doc,
                    if change == "input_write" {
                        &["read"]
                    } else {
                        &["write"]
                    },
                )]))
                .execute(&desk.world.db.pool)
                .await
                .unwrap();
        } else if change == "expired" {
            desk.world.advance(chrono::Duration::hours(2));
        } else if change == "file" {
            sqlx::query(
                "UPDATE document_files SET name='rebound.png' WHERE document_id=$1 AND id=$2",
            )
            .bind(doc)
            .bind(id)
            .execute(&desk.world.db.pool)
            .await
            .unwrap();
        }
        let committed = state(&desk, doc).await;
        blocker.commit().await.unwrap();
        let value = accepted(tokio::time::timeout(WAIT, task).await.unwrap().unwrap());
        if change == "none" || change == "input_write" {
            assert_eq!(value["files"][0]["id"], json!(id));
            assert!(value.get("projection").is_none());
        } else {
            assert_eq!(
                value,
                json!({"projection":"receipt","document_id":doc,"file_ids":[id]})
            );
        }
        assert_eq!(state(&desk, doc).await, committed);
        assert_eq!(probe.puts.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn generated_file_cannot_replace_an_original_missing_lookup_and_limit_labels_need_their_own_read()
 {
    let mut desk = Desk::new().await;
    let doc = document(&desk).await;
    let token = key(
        &desk,
        desk.ada.id(),
        vec![rule(DOCUMENT, "document", doc, &["write", "read"])],
    )
    .await;
    let probe = probe(&mut desk, false);
    let text = format!(
        "![previously missing](image-1.png)

![new](data:image/png;base64,{PICTURE})"
    );
    let full: DocumentConvertReply = serde_json::from_value(accepted(
        call(&desk, &token.secret, &body(Some(doc), &text)).await,
    ))
    .unwrap();
    assert_eq!(full.files.len(), 1);
    assert!(
        full.warnings
            .iter()
            .any(|warning| warning.code == "reader.image_missing")
    );
    let images = full
        .blocks
        .iter()
        .filter_map(|block| match &block.props {
            Some(BlockProps::Image(props)) => Some(props.file_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(images, vec![full.files[0].id]);
    desk.world
        .set_at(
            orch_settings::Target::Node(desk.topic),
            "docs.file_max_bytes",
            json!(8),
        )
        .await;
    let before = state(&desk, doc).await;
    for named in [false, true] {
        let mut rules = vec![rule(DOCUMENT, "document", doc, &["write", "read"])];
        if named {
            rules.push(rule(DOCUMENT, "topic", desk.topic, &["read"]));
        }
        let token = key(&desk, desk.ada.id(), rules).await;
        let reply = call(&desk, &token.secret, &image(doc)).await;
        assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
        let value: Json = serde_json::from_slice(&reply.raw).unwrap();
        assert_eq!(value["value"], 8);
        if named {
            assert_eq!(value["scope"]["id"], json!(desk.topic));
        } else {
            assert!(value["scope"].is_null());
        }
        assert_eq!(state(&desk, doc).await, before);
    }
    assert_eq!(probe.puts.load(Ordering::SeqCst), 3);
}

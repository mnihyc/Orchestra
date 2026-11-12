//! Native attachment-source controls use real HTTP credentials and the actual held store.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use super::*;

use crate::rig::Scene;

use crate::registry::Registry;

use crate::state::AppState;

use crate::testing::RawReply;

use crate::testing::TestApp;

use orch_api_contract::chats::ChatMessageCreate;

use orch_api_contract::chats::ChatMessageCreateBody;

use orch_api_contract::chats::ChatPart;

use orch_api_contract::chats::OrgChatCreate;

use orch_api_contract::chats::OrgChatCreateBody;

use orch_api_contract::documents::DocumentFileCreate;

use orch_api_contract::documents::DocumentFileCreateQuery;

use orch_api_contract::documents::DocumentFileDelete;

use orch_api_contract::documents::NodeDocumentCreate;

use orch_api_contract::documents::NodeDocumentCreateBody;

use orch_blob_contract::store::BlobError;

use orch_blob_contract::store::BlobRead;

use orch_blob_contract::store::BlobStore;

use orch_blob_contract::store::ByteStream;

use orch_blob_contract::store::Stored;

use orch_types::new_id;

use std::sync::Arc;

use std::sync::atomic::AtomicBool;

use std::sync::atomic::AtomicUsize;

use std::sync::atomic::Ordering;

use tokio::sync::Notify;
const WAIT: std::time::Duration = std::time::Duration::from_secs(30);

const TEXT: &[u8] = b"frozen private attachment text\nsecond source line\n";

struct Probe {
    // Retain the native test-owned upload directory for uploads made during the source wait.
    inner: crate::blobs::Blobs,
    hash: Hash,
    used: AtomicBool,
    gets: AtomicUsize,
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
        self.inner.store().put(hash, size, body).await
    }
    async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        let read = self.inner.store().get(hash, range).await;
        if *hash == self.hash {
            self.gets.fetch_add(1, Ordering::SeqCst);
            if !self.used.swap(true, Ordering::SeqCst) {
                self.reached.notify_one();
                self.release.notified().await;
            }
        }
        read
    }
    async fn exists(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.inner.store().exists(hash).await
    }
    async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError> {
        self.inner.store().size(hash).await
    }
    async fn delete(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.inner.store().delete(hash).await
    }
}

fn probe(scene: &mut Scene) -> Arc<Probe> {
    let old = scene.rig.world.app.app.clone();
    let probe = Arc::new(Probe {
        inner: old.blobs.clone(),
        hash: Hash::of(TEXT),
        used: AtomicBool::new(false),
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
        blobs: crate::blobs::Blobs::new(probe.clone(), probe.inner.uploads_dir()),
        cache: crate::cache::Cache::new(&old.config),
        host_ports: old.host_ports.clone(),
        builtin_sources: old.builtin_sources.clone(),
    });
    let mut registry = Registry::new();
    crate::routes::mount(&mut registry);
    scene.rig.world.app = TestApp::over(app, scene.rig.world.app.clock.clone(), registry);
    probe
}

async fn document(scene: &Scene, title: &str) -> Id {
    let request = scene
        .rig
        .world
        .app
        .call(NodeDocumentCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&NodeDocumentCreateBody {
            title: title.into(),
            blocks: None,
            markdown: None,
            access: None,
        });
    request.send().await.ok().into_full().unwrap().base.id
}

async fn file(scene: &Scene, document: Id, bytes: &[u8], overwrite: bool) -> Id {
    let request = scene
        .rig
        .world
        .app
        .call(DocumentFileCreate, &[&document])
        .token(&scene.ada.token)
        .query(&DocumentFileCreateQuery {
            overwrite: Some(overwrite),
        })
        .header(orch_api_contract::threads::FILENAME_HEADER, "source.txt")
        .bytes(bytes);
    request.send().await.ok().into_full().unwrap().id
}

fn query(file: Id) -> ReferencePreviewQuery {
    ReferencePreviewQuery {
        line: Some(1),
        ..whole(InlineRefKind::File, file)
    }
}

fn start(scene: &Scene, file: Id) -> tokio::task::JoinHandle<RawReply> {
    let app = scene.rig.world.app.clone();
    let token = scene.ada.token.clone();
    tokio::spawn(async move {
        let request = app
            .call(ReferencePreviewGet, &[])
            .token(&token)
            .query(&query(file));
        request.send_bytes().await
    })
}

async fn reached(probe: &Probe) {
    tokio::time::timeout(WAIT, probe.reached.notified())
        .await
        .expect("the actual selected blob store must wait");
}

fn hidden(reply: &RawReply) {
    assert!(!String::from_utf8_lossy(&reply.raw).contains("frozen private attachment text"));
    if reply.status == http::StatusCode::OK {
        let card: ReferencePreview = serde_json::from_slice(&reply.raw).unwrap();
        assert!(card.unavailable.is_some());
    } else {
        assert!(matches!(
            reply.status,
            http::StatusCode::UNAUTHORIZED
                | http::StatusCode::FORBIDDEN
                | http::StatusCode::NOT_FOUND
        ));
    }
}

#[cfg(any(test, feature = "test-support"))]
async fn actual_attachment_store_wait_never_substitutes_a_new_document_owner_generation_or_file() {
    for change in ["none", "owner", "overwrite", "missing"] {
        let mut scene = Scene::new().await;
        let owner = document(&scene, "Initial owner").await;
        let other = document(&scene, "Other readable owner").await;
        let id = file(&scene, owner, TEXT, false).await;
        let probe = probe(&mut scene);
        let task = start(&scene, id);
        reached(&probe).await;
        if change == "owner" {
            sqlx::query("UPDATE document_files SET document_id=$2 WHERE id=$1")
                .bind(id)
                .bind(other)
                .execute(scene.rig.db.pool())
                .await
                .unwrap();
        } else if change == "overwrite" {
            file(
                &scene,
                owner,
                b"replacement bytes from the current file\n",
                true,
            )
            .await;
        } else if change == "missing" {
            let request = scene
                .rig
                .world
                .app
                .call(DocumentFileDelete, &[&owner, &id])
                .token(&scene.ada.token);
            request.send().await.ok();
        }
        probe.release.notify_one();
        let reply = tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
        if change == "none" {
            assert_eq!(reply.status, http::StatusCode::OK);
            let card: ReferencePreview = serde_json::from_slice(&reply.raw).unwrap();
            assert!(
                card.part
                    .unwrap()
                    .text
                    .contains("frozen private attachment text")
            );
            assert_eq!(card.file.unwrap().owner_id, Some(owner));
        } else {
            hidden(&reply);
        }
        assert_eq!(
            probe.gets.load(Ordering::SeqCst),
            1,
            "publication never opens replacement source bytes"
        );
        if change == "owner" {
            let request = scene
                .rig
                .world
                .app
                .call(ReferencePreviewGet, &[])
                .token(&scene.ada.token)
                .query(&query(id));
            let fresh = request.send().await.ok();
            assert_eq!(fresh.file.unwrap().owner_id, Some(other));
            assert!(
                fresh
                    .part
                    .unwrap()
                    .text
                    .contains("frozen private attachment text")
            );
            assert_eq!(
                probe.gets.load(Ordering::SeqCst),
                2,
                "a new real request may select the new owner"
            );
        }
    }
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_actual_attachment_store_wait_never_substitutes_a_new_document_owner_generation_or_file()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(self::actual_attachment_store_wait_never_substitutes_a_new_document_owner_generation_or_file())
}

#[cfg(any(test, feature = "test-support"))]
async fn actual_attachment_store_wait_keeps_the_initial_raw_access_deadline_even_if_storage_extends_it()
 {
    for extended in [false, true] {
        let mut scene = Scene::new().await;
        let owner = document(&scene, "Raw deadline owner").await;
        let id = file(&scene, owner, TEXT, false).await;
        let hash = crate::middleware::authentication::token_hash(&scene.ada.token);
        let end = scene.rig.world.now() + chrono::Duration::hours(1);
        sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE token_hash=$1")
            .bind(&hash)
            .bind(end)
            .execute(scene.rig.db.pool())
            .await
            .unwrap();
        let probe = probe(&mut scene);
        let task = start(&scene, id);
        reached(&probe).await;
        scene.rig.world.advance(chrono::Duration::hours(2));
        if extended {
            sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE token_hash=$1")
                .bind(&hash)
                .bind(scene.rig.world.now() + chrono::Duration::days(100))
                .execute(scene.rig.db.pool())
                .await
                .unwrap();
        }
        probe.release.notify_one();
        let reply = tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
        hidden(&reply);
        assert_eq!(reply.status, http::StatusCode::UNAUTHORIZED);
        assert_eq!(probe.gets.load(Ordering::SeqCst), 1);
    }
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_actual_attachment_store_wait_keeps_the_initial_raw_access_deadline_even_if_storage_extends_it()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(self::actual_attachment_store_wait_keeps_the_initial_raw_access_deadline_even_if_storage_extends_it())
}

async fn message(scene: &Scene, chat: Id) -> Id {
    let request = scene
        .rig
        .world
        .app
        .call(ChatMessageCreate, &[&chat])
        .token(&scene.ada.token)
        .body(&ChatMessageCreateBody {
            parts: vec![
                ChatPart::Attachment {
                    blob: Hash::of(TEXT),
                    name: "first.txt".into(),
                    size: i64::try_from(TEXT.len()).unwrap(),
                    media_type: "text/plain".into(),
                },
                ChatPart::Attachment {
                    blob: Hash::of(TEXT),
                    name: "second.txt".into(),
                    size: i64::try_from(TEXT.len()).unwrap(),
                    media_type: "text/plain".into(),
                },
            ],
            reply_to: None,
            agent_review: None,
            client_id: new_id().to_string(),
        });
    request.send().await.ok().into_full().unwrap().message.id
}

#[cfg(any(test, feature = "test-support"))]
async fn actual_chat_attachment_wait_keeps_the_original_message_ordinal_and_material_digest() {
    for change in ["none", "ordinal", "message", "content"] {
        let mut scene = Scene::new().await;
        let owner = document(&scene, "Bytes held natively").await;
        file(&scene, owner, TEXT, false).await;
        let request = scene
            .rig
            .world
            .app
            .call(OrgChatCreate, &[&scene.tree.org.base.id])
            .token(&scene.ada.token)
            .body(&OrgChatCreateBody {
                home_id: None,
                visibility: None,
                user_ids: vec![scene.cai.id()],
                name: Some("Native source chat".into()),
            });
        let chat = request.send().await.ok().into_full().unwrap().base.id;
        let first = message(&scene, chat).await;
        let second = message(&scene, chat).await;
        // Preview GETs materialize these references lazily; sending a message only pins its bytes.
        // Seed the existing reference records so duplicate bytes keep independent ordinal witnesses.
        let inserted = sqlx::query("INSERT INTO chat_file_references (id,chat_id,message_id,carried) VALUES ($1,$2,$3,1),($4,$2,$3,2),($5,$2,$6,1)")
            .bind(new_id()).bind(chat).bind(first).bind(new_id()).bind(new_id()).bind(second)
            .execute(scene.rig.db.pool()).await.unwrap().rows_affected();
        assert_eq!(inserted, 3);
        let id: Id = sqlx::query_scalar(
            "SELECT id FROM chat_file_references WHERE chat_id=$1 AND message_id=$2 AND carried=1",
        )
        .bind(chat)
        .bind(first)
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap();
        let probe = probe(&mut scene);
        let task = start(&scene, id);
        reached(&probe).await;
        if change == "ordinal" {
            let other: Id = sqlx::query_scalar(
                "SELECT id FROM chat_file_references WHERE message_id=$1 AND carried=2",
            )
            .bind(first)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap();
            let mut tx = scene.rig.db.pool().begin().await.unwrap();
            sqlx::query("UPDATE chat_file_references SET carried=3 WHERE id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await
                .unwrap();
            sqlx::query("UPDATE chat_file_references SET carried=1 WHERE id=$1")
                .bind(other)
                .execute(&mut *tx)
                .await
                .unwrap();
            sqlx::query("UPDATE chat_file_references SET carried=2 WHERE id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        } else if change == "message" {
            let request = scene
                .rig
                .world
                .app
                .call(ChatMessageCreate, &[&chat])
                .token(&scene.ada.token)
                .body(&ChatMessageCreateBody {
                    parts: vec![ChatPart::Text {
                        text: "Temporary native swap anchor".into(),
                    }],
                    reply_to: None,
                    agent_review: None,
                    client_id: new_id().to_string(),
                });
            let temporary = request.send().await.ok().into_full().unwrap().message.id;
            let other: Id = sqlx::query_scalar(
                "SELECT id FROM chat_file_references WHERE message_id=$1 AND carried=1",
            )
            .bind(second)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap();
            let mut tx = scene.rig.db.pool().begin().await.unwrap();
            sqlx::query("UPDATE chat_file_references SET message_id=$2 WHERE id=$1")
                .bind(id)
                .bind(temporary)
                .execute(&mut *tx)
                .await
                .unwrap();
            sqlx::query("UPDATE chat_file_references SET message_id=$2 WHERE id=$1")
                .bind(other)
                .bind(first)
                .execute(&mut *tx)
                .await
                .unwrap();
            sqlx::query("UPDATE chat_file_references SET message_id=$2 WHERE id=$1")
                .bind(id)
                .bind(second)
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        } else if change == "content" {
            sqlx::query("UPDATE chat_messages SET content=jsonb_set(content,'{0,name}',to_jsonb('renamed-source.txt'::text)) WHERE chat_id=$1 AND id=$2").bind(chat).bind(first).execute(scene.rig.db.pool()).await.unwrap();
        }
        probe.release.notify_one();
        let reply = tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
        if change == "none" {
            assert_eq!(reply.status, http::StatusCode::OK);
            let card: ReferencePreview = serde_json::from_slice(&reply.raw).unwrap();
            assert_eq!(card.file.unwrap().message, Some(first));
            assert!(
                card.part
                    .unwrap()
                    .text
                    .contains("frozen private attachment text")
            );
        } else {
            hidden(&reply);
        }
        assert_eq!(
            probe.gets.load(Ordering::SeqCst),
            1,
            "the native store is never replayed for a replacement attachment witness"
        );
    }
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_actual_chat_attachment_wait_keeps_the_original_message_ordinal_and_material_digest()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::actual_chat_attachment_wait_keeps_the_original_message_ordinal_and_material_digest(),
    )
}

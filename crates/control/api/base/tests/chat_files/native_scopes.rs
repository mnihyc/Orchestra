//! Scoped native file metadata and immutable-source delivery use the same chat readers.

use super::*;
use chrono::Duration;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::threads::{UploadBlobGet, UploadBlobGetQuery};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity;
use orch_types::Json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::Notify;

#[path = "attachment_scopes.rs"]
mod attachment_scopes;

fn rule(operation: &str, kind: &str, id: Id, resource: &str, action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[resource],
        "allow":[{"operation":operation,"actions":[action]}]})
}

async fn key(room: &Room, actor: &Who, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *room.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: actor.id(),
            name: "Native chat file fixture",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: room.world.now(),
            expires_at: Some(room.world.now() + Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    secret
}

async fn read(room: &Room, token: &str, chat: Id, hash: Hash) -> RawReply {
    room.world
        .app
        .call(ChatBlobGet, &[&chat, &hash])
        .token(token)
        .query(&ChatBlobGetQuery::default())
        .send_bytes()
        .await
}

#[tokio::test]
async fn chat_blob_read_binds_its_actual_chat_and_preserves_native_bytes_ranges_and_headers() {
    let room = Room::new().await;
    let bytes = pdf(128);
    let stored = upload(&room, &room.bea, room.chat, "native-private.pdf", &bytes)
        .await
        .ok();
    send_file(&room, &room.bea, room.chat, &stored).await;
    let different = room.direct(&room.cai, &room.bea).await.base.id;
    for refused in [
        rule("ChatBlobGet", "chat", different, "chat", "read"),
        rule("ChatBlobGet", "chat", room.chat, "chat", "write"),
        rule("ChatGet", "chat", room.chat, "chat", "read"),
        rule("ChatBlobGet", "chat", room.chat, "thread", "read"),
    ] {
        let token = key(&room, &room.cai, vec![refused]).await;
        let denied = read(&room, &token, room.chat, stored.blob).await;
        assert!(!denied.status.is_success());
        assert!(
            !denied
                .raw
                .windows(bytes.len())
                .any(|part| part == bytes.as_slice())
        );
        assert!(!denied.headers.contains_key("etag"));
        assert!(!denied.headers.contains_key("content-disposition"));
    }
    let token = key(
        &room,
        &room.cai,
        vec![rule("ChatBlobGet", "chat", room.chat, "chat", "read")],
    )
    .await;
    let ordinary = fetch(&room, &room.cai, room.chat, &stored.blob, None).await;
    let scoped = read(&room, &token, room.chat, stored.blob).await;
    assert_eq!(scoped.status, StatusCode::OK);
    assert_eq!(scoped.raw, ordinary.raw);
    for name in [
        "content-type",
        "content-disposition",
        "cache-control",
        "etag",
        "x-content-type-options",
        "content-security-policy",
    ] {
        assert_eq!(
            scoped.headers.get(name),
            ordinary.headers.get(name),
            "{name}"
        );
    }
    let ranged = room
        .world
        .app
        .call(ChatBlobGet, &[&room.chat, &stored.blob])
        .token(&token)
        .query(&ChatBlobGetQuery::default())
        .header("range", "bytes=10-19")
        .send_bytes()
        .await;
    assert_eq!(ranged.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(ranged.raw.as_ref(), &bytes[10..20]);
    let legacy = new_token(API_TOKEN_PREFIX);
    identity::create_api_token(
        &mut *room.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: room.cai.id(),
            name: "Legacy native chat file",
            token_hash: &token_hash(&legacy),
            scopes: &["chat.view".into(), "topic.view".into()],
            node_id: None,
            at: room.world.now(),
            expires_at: Some(room.world.now() + Duration::hours(1)),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        read(&room, &legacy, room.chat, stored.blob).await.raw,
        ordinary.raw
    );
    let preview = ChatBlobGetQuery {
        preview: Some(true),
        ..Default::default()
    };
    let native_preview = room
        .world
        .app
        .call(ChatBlobGet, &[&room.chat, &stored.blob])
        .token(&room.cai.token)
        .query(&preview)
        .send_bytes()
        .await;
    let key_preview = room
        .world
        .app
        .call(ChatBlobGet, &[&room.chat, &stored.blob])
        .token(&token)
        .query(&preview)
        .send_bytes()
        .await;
    assert_eq!(key_preview.status, native_preview.status);
    assert_eq!(key_preview.raw, native_preview.raw);
    assert!(key_preview.headers.contains_key("x-orch-file-id"));
    assert_eq!(
        key_preview.headers["x-orch-file-id"],
        native_preview.headers["x-orch-file-id"]
    );
    let stranger = key(
        &room,
        &room.eve,
        vec![rule("ChatBlobGet", "chat", room.chat, "chat", "read")],
    )
    .await;
    let denied = read(&room, &stranger, room.chat, stored.blob).await;
    assert!(
        !denied.status.is_success(),
        "scope never supplies ordinary chat reading"
    );
}

#[tokio::test]
async fn chat_blob_private_drafts_keep_their_exact_own_operation_byte_source_read() {
    use orch_api_contract::chats::{ChatDraftSet, ChatDraftSetBody};
    let room = Room::new().await;
    let stored = upload(
        &room,
        &room.bea,
        room.chat,
        "private-draft.txt",
        b"only the saved private draft",
    )
    .await
    .ok();
    let part = ChatPart::Attachment {
        blob: stored.blob,
        name: stored.name.clone(),
        size: stored.size,
        media_type: stored.media_type.clone(),
    };
    room.world
        .app
        .call(ChatDraftSet, &[&room.chat])
        .token(&room.bea.token)
        .body(&ChatDraftSetBody {
            parts: vec![part],
            reply_to: None,
        })
        .send()
        .await
        .ok();
    let chat = rule("ChatBlobGet", "chat", room.chat, "chat", "read");
    for extra in [
        None,
        Some(rule(
            "UploadBlobGet",
            "account",
            room.bea.id(),
            "upload",
            "read",
        )),
    ] {
        let mut rules = vec![chat.clone()];
        rules.extend(extra);
        let token = key(&room, &room.bea, rules).await;
        assert!(
            !read(&room, &token, room.chat, stored.blob)
                .await
                .status
                .is_success()
        );
    }
    let token = key(
        &room,
        &room.bea,
        vec![
            chat.clone(),
            rule("ChatBlobGet", "account", room.bea.id(), "upload", "read"),
        ],
    )
    .await;
    let body = read(&room, &token, room.chat, stored.blob).await;
    assert_eq!(body.status, StatusCode::OK);
    assert_eq!(body.raw.as_ref(), b"only the saved private draft");
    let another = key(
        &room,
        &room.cai,
        vec![
            chat.clone(),
            rule("ChatBlobGet", "account", room.bea.id(), "upload", "read"),
        ],
    )
    .await;
    assert!(
        !read(&room, &another, room.chat, stored.blob)
            .await
            .status
            .is_success()
    );
    send_file(&room, &room.bea, room.chat, &stored).await;
    let saved_only = key(&room, &room.cai, vec![chat]).await;
    let saved = read(&room, &saved_only, room.chat, stored.blob).await;
    assert_eq!(saved.status, StatusCode::OK);
    assert_eq!(
        saved.raw, body.raw,
        "a saved message is an independent native chat source, with no private custody loan"
    );
}

use orch_blob_contract::range::ByteRange;
use orch_blob_contract::store::{BlobError, BlobRead, BlobStore, ByteStream, Stored};

struct OpenGate {
    inner: Arc<dyn BlobStore>,
    directory: tempfile::TempDir,
    hash: Hash,
    used: AtomicBool,
    opens: AtomicUsize,
    reached: Notify,
    release: Notify,
}
#[async_trait::async_trait]
impl BlobStore for OpenGate {
    async fn put(
        &self,
        hash: &Hash,
        size: Option<i64>,
        body: ByteStream,
    ) -> Result<Stored, BlobError> {
        self.inner.put(hash, size, body).await
    }
    async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        let opened = self.inner.get(hash, range).await?;
        if *hash == self.hash {
            self.opens.fetch_add(1, Ordering::SeqCst);
        }
        if *hash == self.hash && !self.used.swap(true, Ordering::SeqCst) {
            assert_eq!(opened.meta.hash, self.hash);
            assert!(opened.meta.size > 0);
            self.reached.notify_one();
            self.release.notified().await;
        }
        Ok(opened)
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

fn gate_open(room: &mut Room, hash: Hash) -> Arc<OpenGate> {
    use orch_api_core::{registry::Registry, state::AppState, testing::TestApp};
    let old = room.world.app.app.clone();
    let gate = Arc::new(OpenGate {
        inner: old.blobs.store(),
        directory: tempfile::tempdir().unwrap(),
        hash,
        used: AtomicBool::new(false),
        opens: AtomicUsize::new(0),
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
    room.world.app = TestApp::over(state, room.world.app.clock.clone(), registry);
    gate
}

const WAIT: std::time::Duration = std::time::Duration::from_secs(30);

#[tokio::test]
async fn chat_blob_delivery_rechecks_the_actual_key_after_its_native_store_opens() {
    for expire in [false, true] {
        let mut room = Room::new().await;
        let stored = upload(
            &room,
            &room.bea,
            room.chat,
            "opened-private.txt",
            b"opened immutable message bytes",
        )
        .await
        .ok();
        send_file(&room, &room.bea, room.chat, &stored).await;
        let token = key(
            &room,
            &room.cai,
            vec![rule("ChatBlobGet", "chat", room.chat, "chat", "read")],
        )
        .await;
        let gate = gate_open(&mut room, stored.blob);
        let request = read(&room, &token, room.chat, stored.blob);
        let change = async {
            tokio::time::timeout(WAIT, gate.reached.notified())
                .await
                .expect("the actual immutable file is opened");
            if expire {
                room.world.advance(Duration::hours(2));
            }
            gate.release.notify_one();
        };
        let (reply, ()) = tokio::time::timeout(WAIT, async { tokio::join!(request, change) })
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
        if expire {
            assert!(!reply.headers.contains_key("etag"));
            assert!(!reply.headers.contains_key("content-disposition"));
            assert!(
                !String::from_utf8_lossy(&reply.raw).contains("opened immutable message bytes")
            );
        } else {
            assert_eq!(reply.raw.as_ref(), b"opened immutable message bytes");
        }
    }
}

#[tokio::test]
async fn chat_blob_publication_cannot_replace_its_message_with_a_new_same_hash_source() {
    let mut room = Room::new().await;
    let stored = upload(
        &room,
        &room.bea,
        room.chat,
        "old-message-source.txt",
        b"bytes held from the original message",
    )
    .await
    .ok();
    let original = send_file(&room, &room.bea, room.chat, &stored).await;
    let token = key(
        &room,
        &room.cai,
        vec![rule("ChatBlobGet", "chat", room.chat, "chat", "read")],
    )
    .await;
    let gate = gate_open(&mut room, stored.blob);
    let request = read(&room, &token, room.chat, stored.blob);
    let change = async {
        tokio::time::timeout(WAIT, gate.reached.notified())
            .await
            .expect("the original message's file has actually opened");
        room.world
            .app
            .call(ChatMessageDelete, &[&room.chat, &original.id])
            .token(&room.bea.token)
            .send()
            .await
            .ok();
        let replacement = send_file(&room, &room.bea, room.chat, &stored).await;
        assert_ne!(replacement.id, original.id);
        gate.release.notify_one();
    };
    let (held, ()) = tokio::time::timeout(WAIT, async { tokio::join!(request, change) })
        .await
        .unwrap();
    assert!(!held.status.is_success());
    assert!(!held.headers.contains_key("etag"));
    assert!(!String::from_utf8_lossy(&held.raw).contains("bytes held from the original message"));
    let current = read(&room, &token, room.chat, stored.blob).await;
    assert_eq!(current.status, StatusCode::OK);
    assert_eq!(
        current.raw.as_ref(),
        b"bytes held from the original message"
    );
}

async fn read_upload(room: &Room, token: &str, hash: Hash, name: &str) -> RawReply {
    room.world
        .app
        .call(UploadBlobGet, &[&hash])
        .token(token)
        .query(&UploadBlobGetQuery {
            cursor: None,
            q: None,
            limit: None,
            name: name.to_owned(),
            archive: None,
            inside: None,
            entry: None,
            preview: None,
            download: None,
        })
        .send_bytes()
        .await
}

#[tokio::test]
async fn upload_blob_read_binds_actual_account_custody_and_preserves_native_ranges_and_headers() {
    let room = Room::new().await;
    let bytes = pdf(128);
    let stored = upload(&room, &room.bea, room.chat, "personal-source.pdf", &bytes)
        .await
        .ok();
    let mut membership = rule("UploadBlobGet", "org", room.root, "upload", "read");
    membership["target"]["reach"] = json!("subtree");
    for refused in [
        rule("UploadBlobGet", "account", room.cai.id(), "upload", "read"),
        rule("UploadBlobGet", "account", room.bea.id(), "upload", "write"),
        rule("UploadBlobGet", "account", room.bea.id(), "thread", "read"),
        rule("ChatBlobGet", "account", room.bea.id(), "upload", "read"),
        membership,
    ] {
        let token = key(&room, &room.bea, vec![refused]).await;
        let denied = read_upload(&room, &token, stored.blob, &stored.name).await;
        assert!(!denied.status.is_success());
        assert!(!denied.headers.contains_key("etag"));
        assert!(!denied.headers.contains_key("content-disposition"));
        assert!(
            !denied
                .raw
                .windows(bytes.len())
                .any(|part| part == bytes.as_slice())
        );
    }
    let token = key(
        &room,
        &room.bea,
        vec![rule(
            "UploadBlobGet",
            "account",
            room.bea.id(),
            "upload",
            "read",
        )],
    )
    .await;
    let native = read_upload(&room, &room.bea.token, stored.blob, &stored.name).await;
    let scoped = read_upload(&room, &token, stored.blob, &stored.name).await;
    assert_eq!(scoped.status, StatusCode::OK);
    assert_eq!(scoped.raw, native.raw);
    assert_eq!(scoped.raw.as_ref(), bytes.as_slice());
    for header in [
        "content-type",
        "content-disposition",
        "cache-control",
        "etag",
        "x-content-type-options",
        "content-security-policy",
    ] {
        assert_eq!(
            scoped.headers.get(header),
            native.headers.get(header),
            "{header}"
        );
    }
    let ranged = room
        .world
        .app
        .call(UploadBlobGet, &[&stored.blob])
        .token(&token)
        .query(&UploadBlobGetQuery {
            cursor: None,
            q: None,
            limit: None,
            name: stored.name.clone(),
            archive: None,
            inside: None,
            entry: None,
            preview: None,
            download: None,
        })
        .header("range", "bytes=10-19")
        .send_bytes()
        .await;
    assert_eq!(ranged.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(ranged.raw.as_ref(), &bytes[10..20]);
}

#[tokio::test]
async fn upload_blob_saved_source_needs_its_own_operation_read_and_current_native_visibility() {
    let room = Room::new().await;
    let stored = upload(
        &room,
        &room.bea,
        room.chat,
        "saved-source.txt",
        b"saved chat source bytes",
    )
    .await
    .ok();
    send_file(&room, &room.bea, room.chat, &stored).await;
    for rules in [
        vec![rule(
            "UploadBlobGet",
            "account",
            room.cai.id(),
            "upload",
            "read",
        )],
        vec![rule("ChatBlobGet", "chat", room.chat, "chat", "read")],
    ] {
        let token = key(&room, &room.cai, rules).await;
        assert!(
            !read_upload(&room, &token, stored.blob, &stored.name)
                .await
                .status
                .is_success()
        );
    }
    let token = key(
        &room,
        &room.cai,
        vec![rule("UploadBlobGet", "chat", room.chat, "chat", "read")],
    )
    .await;
    let reply = read_upload(&room, &token, stored.blob, &stored.name).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.raw.as_ref(), b"saved chat source bytes");
    let stranger = key(
        &room,
        &room.eve,
        vec![rule("UploadBlobGet", "chat", room.chat, "chat", "read")],
    )
    .await;
    assert!(
        !read_upload(&room, &stranger, stored.blob, &stored.name)
            .await
            .status
            .is_success()
    );
    let legacy = new_token(API_TOKEN_PREFIX);
    identity::create_api_token(
        &mut *room.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: room.cai.id(),
            name: "Legacy native upload reader",
            token_hash: &token_hash(&legacy),
            scopes: &["chat.view".into(), "topic.view".into()],
            node_id: None,
            at: room.world.now(),
            expires_at: Some(room.world.now() + Duration::hours(1)),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        read_upload(&room, &legacy, stored.blob, &stored.name)
            .await
            .raw,
        reply.raw
    );
}

#[tokio::test]
async fn upload_blob_delivery_rechecks_the_actual_key_after_the_native_store_opens() {
    for expire in [false, true] {
        let mut room = Room::new().await;
        let stored = upload(
            &room,
            &room.bea,
            room.chat,
            "opened-upload.txt",
            b"opened personal upload bytes",
        )
        .await
        .ok();
        let token = key(
            &room,
            &room.bea,
            vec![rule(
                "UploadBlobGet",
                "account",
                room.bea.id(),
                "upload",
                "read",
            )],
        )
        .await;
        let gate = gate_open(&mut room, stored.blob);
        let request = read_upload(&room, &token, stored.blob, &stored.name);
        let change = async {
            tokio::time::timeout(WAIT, gate.reached.notified())
                .await
                .expect("the original native upload file opened");
            if expire {
                room.world.advance(Duration::hours(2));
            }
            gate.release.notify_one();
        };
        let (reply, ()) = tokio::time::timeout(WAIT, async { tokio::join!(request, change) })
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
        if expire {
            assert!(!reply.headers.contains_key("etag"));
            assert!(!reply.headers.contains_key("content-disposition"));
            assert!(!String::from_utf8_lossy(&reply.raw).contains("opened personal upload bytes"));
        } else {
            assert_eq!(reply.raw.as_ref(), b"opened personal upload bytes");
        }
    }
}

#[tokio::test]
async fn upload_blob_publication_cannot_replace_its_selected_chat_with_another_same_hash_source() {
    let mut room = Room::new().await;
    let old = room.direct(&room.bea, &room.cai).await.base.id;
    let another = room
        .group(&room.bea, &[&room.cai], Some("Other byte source"))
        .await
        .base
        .id;
    let stored = upload(
        &room,
        &room.bea,
        old,
        "selected-native-source.txt",
        b"the original selected hash bytes",
    )
    .await
    .ok();
    let original = send_file(&room, &room.bea, old, &stored).await;
    let token = key(
        &room,
        &room.cai,
        vec![
            rule("UploadBlobGet", "chat", old, "chat", "read"),
            rule("UploadBlobGet", "chat", another, "chat", "read"),
        ],
    )
    .await;
    let gate = gate_open(&mut room, stored.blob);
    let request = read_upload(&room, &token, stored.blob, &stored.name);
    let change = async {
        tokio::time::timeout(WAIT, gate.reached.notified())
            .await
            .expect("the chosen saved source's bytes opened");
        room.world
            .app
            .call(ChatMessageDelete, &[&old, &original.id])
            .token(&room.bea.token)
            .send()
            .await
            .ok();
        send_file(&room, &room.bea, another, &stored).await;
        gate.release.notify_one();
    };
    let (held, ()) = tokio::time::timeout(WAIT, async { tokio::join!(request, change) })
        .await
        .unwrap();
    assert!(!held.status.is_success());
    assert!(!held.headers.contains_key("etag"));
    assert!(!String::from_utf8_lossy(&held.raw).contains("the original selected hash bytes"));
    let fresh = read_upload(&room, &token, stored.blob, &stored.name).await;
    assert_eq!(fresh.status, StatusCode::OK);
    assert_eq!(fresh.raw.as_ref(), b"the original selected hash bytes");
}

#[tokio::test]
async fn upload_blob_preview_and_archive_forms_keep_the_same_native_source_read() {
    use std::io::Write;
    let room = Room::new().await;
    let token = key(
        &room,
        &room.bea,
        vec![rule(
            "UploadBlobGet",
            "account",
            room.bea.id(),
            "upload",
            "read",
        )],
    )
    .await;
    let stored = upload(&room, &room.bea, room.chat, "preview-source.pdf", &pdf(128))
        .await
        .ok();
    let preview = UploadBlobGetQuery {
            cursor: None,
            q: None,
            limit: None,
        name: stored.name.clone(),
        archive: None,
        inside: None,
        entry: None,
        preview: Some(true),
        download: None,
    };
    let native = room
        .world
        .app
        .call(UploadBlobGet, &[&stored.blob])
        .token(&room.bea.token)
        .query(&preview)
        .send_bytes()
        .await;
    let scoped = room
        .world
        .app
        .call(UploadBlobGet, &[&stored.blob])
        .token(&token)
        .query(&preview)
        .send_bytes()
        .await;
    assert_eq!(scoped.status, native.status);
    assert_eq!(scoped.raw, native.raw);
    assert_eq!(
        scoped.headers.get("x-orch-file-id"),
        native.headers.get("x-orch-file-id")
    );
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file(
        "doc.txt",
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    zip.write_all(b"inside the selected native archive")
        .unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    let stored = upload(&room, &room.bea, room.chat, "source.zip", &bytes)
        .await
        .ok();
    for entry in [None, Some("doc.txt".to_owned())] {
        let archive = UploadBlobGetQuery {
            cursor: None,
            q: None,
            limit: None,
            name: stored.name.clone(),
            archive: Some(true),
            inside: None,
            entry,
            preview: None,
            download: None,
        };
        let native = room
            .world
            .app
            .call(UploadBlobGet, &[&stored.blob])
            .token(&room.bea.token)
            .query(&archive)
            .send_bytes()
            .await;
        let scoped = room
            .world
            .app
            .call(UploadBlobGet, &[&stored.blob])
            .token(&token)
            .query(&archive)
            .send_bytes()
            .await;
        assert_eq!(scoped.status, StatusCode::OK);
        assert_eq!(scoped.status, native.status);
        assert_eq!(scoped.raw, native.raw);
        if archive.entry.is_some() {
            assert_eq!(scoped.raw.as_ref(), b"inside the selected native archive");
        }
    }
}

//! Upload inventory keeps private custody, administrative opening and physical bytes distinct.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod chat_world;
mod places;
mod support;

use chrono::Duration;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::{
    chats::{ChatDraftSet, ChatDraftSetBody, ChatPart},
    common::{Page, PageQuery},
    storage::{StorageGet, StorageQuery},
    threads::{
        AdminUploadContentGet, FILENAME_HEADER, MeUploadList, UploadBlobDelete,
        UploadBlobDeleteQuery, UploadBlobGetQuery, UploadCreate, UploadInventoryItem,
        UploadInventoryKind, UploadInventoryState, UploadSessionComplete, UploadSessionCreate,
        UploadSessionCreateBody, UploadSessionDelete, UploadSessionPut, UploadSessionPutQuery,
        UploadedBlob,
    },
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::{identity, storage, uploads};
use orch_types::{Hash, Id, Json, new_id};
use places::Who;
use serde_json::json;
use support::World;

async fn whole(world: &World, owner: &Who, name: &str, bytes: &[u8]) -> UploadedBlob {
    world
        .app
        .call(UploadCreate, &[])
        .token(&owner.token)
        .header("content-type", "image/png")
        .header(FILENAME_HEADER, name)
        .bytes(bytes)
        .send()
        .await
        .ok()
}
async fn list(world: &World, token: &str, query: &PageQuery) -> Page<UploadInventoryItem> {
    world
        .app
        .call(MeUploadList, &[])
        .token(token)
        .query(query)
        .send()
        .await
        .ok()
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
            name: "Upload inventory scope control",
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
fn rule(op: &str, actor: Id, resource: &str, action: &str) -> Json {
    json!({"target":{"kind":"account","id":actor,"reach":"self"},"resources":[resource],
        "allow":[{"operation":op,"actions":[action]}]})
}

#[tokio::test]
async fn own_unattached_uploads_keep_their_uploader_time_and_other_peoples_same_hash_custody() {
    let room = chat_world::Room::new().await;
    let bytes = b"two real uploaders of one immutable body";
    let mine = whole(&room.world, &room.bea, "mine.png", bytes).await;
    let theirs = whole(&room.world, &room.cai, "theirs.png", bytes).await;
    assert_eq!(mine.blob, theirs.blob);
    room.world
        .app
        .call(ChatDraftSet, &[&room.chat])
        .token(&room.cai.token)
        .body(&ChatDraftSetBody {
            parts: vec![ChatPart::Attachment {
                blob: theirs.blob,
                name: theirs.name,
                size: theirs.size,
                media_type: theirs.media_type,
            }],
            reply_to: None,
        })
        .send()
        .await
        .ok();
    let page = list(&room.world, &room.bea.token, &PageQuery::default()).await;
    let row = page
        .items
        .iter()
        .find(|row| row.blob == Some(mine.blob))
        .unwrap();
    assert_eq!(row.uploader.as_ref().unwrap().id, room.bea.id());
    assert_eq!(row.name.as_deref(), Some("mine.png"));
    assert_eq!(row.created_at.get(), room.world.now());
    assert_eq!(row.attachment_count, 0);
    assert!(row.attached_to.is_empty());
    assert!(row.can_delete);
    assert!(!row.can_open);
    assert!(row.admin_access.is_none());
    let other = list(&room.world, &room.cai.token, &PageQuery::default()).await;
    assert!(!other.items.iter().any(|row| row.blob == Some(mine.blob)));
    let admin = places::admin(&room.world, "attached-upload-admin").await;
    let report = room
        .world
        .app
        .call(StorageGet, &[])
        .token(&admin.token)
        .query(&StorageQuery {
            scope: Some("system".into()),
            kind: Some(orch_api_contract::storage::StorageKind::Upload),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let attached: Vec<_> = report
        .items
        .iter()
        .filter_map(|row| row.upload.as_ref())
        .filter(|upload| upload.blob == Some(mine.blob))
        .collect();
    assert_eq!(attached.len(), 2);
    assert!(attached.iter().all(|upload| {
        upload.attachment_count == 1
            && upload
                .attached_to
                .iter()
                .any(|source| source.kind == "chat_draft" && source.id == room.chat)
    }));
    room.world
        .app
        .call(UploadBlobDelete, &[&mine.blob])
        .token(&room.bea.token)
        .query(&UploadBlobDeleteQuery::default())
        .send()
        .await
        .ok();
    assert!(
        !uploads::owns(&mut *room.world.conn().await, room.bea.id(), &mine.blob)
            .await
            .unwrap()
    );
    assert!(
        uploads::owns(&mut *room.world.conn().await, room.cai.id(), &mine.blob)
            .await
            .unwrap()
    );
    assert!(
        room.world
            .app
            .app
            .blobs
            .store()
            .exists(&mine.blob)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn upload_inventory_uses_its_own_read_names_and_delete_capability_on_the_actual_account() {
    let world = World::new().await;
    let owner = places::person(&world, "upload-owner").await;
    let stranger = places::person(&world, "upload-stranger").await;
    whole(
        &world,
        &owner,
        "private.png",
        b"kept private upload metadata",
    )
    .await;
    let reader = key(
        &world,
        owner.id(),
        vec![rule("MeUploadList", owner.id(), "upload", "read")],
    )
    .await;
    let page = list(&world, &reader, &PageQuery::default()).await;
    assert_eq!(page.items.len(), 1);
    assert!(page.items[0].uploader.is_none());
    assert!(!page.items[0].can_delete);
    let named = key(
        &world,
        owner.id(),
        vec![
            rule("MeUploadList", owner.id(), "upload", "read"),
            rule("MeUploadList", owner.id(), "account", "read"),
            rule("UploadBlobDelete", owner.id(), "upload", "write"),
        ],
    )
    .await;
    let named = list(&world, &named, &PageQuery::default()).await;
    assert_eq!(named.items[0].uploader.as_ref().unwrap().id, owner.id());
    assert!(named.items[0].can_delete);
    for rules in [
        vec![rule("MeUploadList", stranger.id(), "upload", "read")],
        vec![rule("MeUploadList", owner.id(), "upload", "write")],
        vec![rule("UploadBlobGet", owner.id(), "upload", "read")],
    ] {
        let refused = key(&world, owner.id(), rules).await;
        let reply = world
            .app
            .call(MeUploadList, &[])
            .token(&refused)
            .send_bytes()
            .await;
        assert_eq!(
            reply.status,
            StatusCode::FORBIDDEN,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        assert!(!String::from_utf8_lossy(&reply.raw).contains("private.png"));
    }
}

#[tokio::test]
async fn personal_upload_pages_keep_their_actual_owner_and_do_not_repeat_or_skip_unattached_rows() {
    let world = World::new().await;
    let owner = places::person(&world, "paged-upload-owner").await;
    let other = places::person(&world, "paged-upload-other").await;
    for body in [
        b"first".as_slice(),
        b"second".as_slice(),
        b"third".as_slice(),
    ] {
        whole(&world, &owner, "file.png", body).await;
    }
    let mut query = PageQuery {
        limit: Some(1),
        cursor: None,
    };
    let mut ids = std::collections::HashSet::new();
    let mut first_cursor = None;
    loop {
        let page = list(&world, &owner.token, &query).await;
        assert_eq!(page.items.len(), 1);
        assert!(ids.insert(page.items[0].id.clone()));
        if first_cursor.is_none() {
            first_cursor = page.next_cursor.clone();
        }
        query.cursor = page.next_cursor;
        if query.cursor.is_none() {
            break;
        }
    }
    assert_eq!(ids.len(), 3);
    let refused = world
        .app
        .call(MeUploadList, &[])
        .token(&other.token)
        .query(&PageQuery {
            limit: Some(1),
            cursor: first_cursor,
        })
        .send_bytes()
        .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_partial_upload_reports_real_progress_time_and_can_be_cancelled_after_expiry_without_late_writes()
 {
    let world = World::new().await;
    let owner = places::person(&world, "partial-upload-owner").await;
    let upload = world
        .app
        .call(UploadSessionCreate, &[])
        .token(&owner.token)
        .body(&UploadSessionCreateBody {
            size: 4,
            name: "partial.bin".into(),
            media_type: None,
        })
        .send()
        .await
        .ok();
    world.advance(Duration::seconds(1));
    world
        .app
        .call(UploadSessionPut, &[&upload.upload_id])
        .token(&owner.token)
        .query(&UploadSessionPutQuery { offset: 0 })
        .bytes(b"ab")
        .send()
        .await
        .ok();
    let page = list(&world, &owner.token, &PageQuery::default()).await;
    let row = page
        .items
        .iter()
        .find(|row| row.session_id == Some(upload.upload_id))
        .unwrap();
    assert_eq!(row.kind, UploadInventoryKind::Session);
    assert_eq!(row.received, 2);
    assert_eq!(row.updated_at.get(), world.now());
    assert!(row.updated_at > row.created_at);
    assert!(
        world
            .app
            .app
            .blobs
            .part_of_session(upload.upload_id)
            .exists()
    );
    world.advance(Duration::days(2));
    let page = list(&world, &owner.token, &PageQuery::default()).await;
    let row = page
        .items
        .iter()
        .find(|row| row.session_id == Some(upload.upload_id))
        .unwrap();
    assert_eq!(row.state, UploadInventoryState::Expired);
    assert!(row.can_delete);
    let append = world
        .app
        .call(UploadSessionPut, &[&upload.upload_id])
        .token(&owner.token)
        .query(&UploadSessionPutQuery { offset: 2 })
        .bytes(b"cd")
        .send_bytes()
        .await;
    assert_eq!(append.status, StatusCode::GONE);
    let complete = world
        .app
        .call(UploadSessionComplete, &[&upload.upload_id])
        .token(&owner.token)
        .send_bytes()
        .await;
    assert_eq!(complete.status, StatusCode::GONE);
    world
        .app
        .call(UploadSessionDelete, &[&upload.upload_id])
        .token(&owner.token)
        .send()
        .await
        .ok();
    assert!(
        !world
            .app
            .app
            .blobs
            .part_of_session(upload.upload_id)
            .exists()
    );
    assert!(
        list(&world, &owner.token, &PageQuery::default())
            .await
            .items
            .is_empty()
    );
}

#[tokio::test]
async fn system_upload_inventory_records_owners_partials_and_shared_bytes_and_admin_opening_is_audited_once()
 {
    let world = World::new().await;
    let owner = places::person(&world, "inventory-owner").await;
    let other = places::person(&world, "inventory-other").await;
    let admin = places::admin(&world, "inventory-admin").await;
    let bytes = b"shared bytes for the system inventory";
    let blob = whole(&world, &owner, "original.png", bytes).await;
    whole(&world, &other, "another.png", bytes).await;
    let partial = world
        .app
        .call(UploadSessionCreate, &[])
        .token(&owner.token)
        .body(&UploadSessionCreateBody {
            size: 20,
            name: "unfinished.dat".into(),
            media_type: None,
        })
        .send()
        .await
        .ok();
    let machine = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut *world.conn().await,
        "machine",
        Some(owner.id()),
        None,
        "upload-source",
    )
    .await
    .unwrap();
    let machine_hash = Hash::of(b"a machine transfer that is not yet complete");
    storage::begin_upload(
        &mut *world.conn().await,
        &machine_hash,
        i64::try_from(b"a machine transfer that is not yet complete".len()).unwrap(),
        Some(machine),
        world.now(),
    )
    .await
    .unwrap();
    let report = world
        .app
        .call(StorageGet, &[])
        .token(&admin.token)
        .query(&StorageQuery {
            scope: Some("system".into()),
            page: PageQuery {
                limit: Some(orch_api_contract::common::MAX_LIMIT),
                cursor: None,
            },
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let completed: Vec<_> = report
        .items
        .iter()
        .filter(|row| {
            row.upload.as_ref().is_some_and(|upload| {
                upload.blob == Some(blob.blob) && upload.kind == UploadInventoryKind::Completed
            })
        })
        .collect();
    assert_eq!(completed.len(), 2);
    assert!(
        completed
            .iter()
            .any(|row| row.owner.as_ref().unwrap().id == owner.id())
    );
    assert!(
        completed
            .iter()
            .any(|row| row.owner.as_ref().unwrap().id == other.id())
    );
    assert!(
        completed
            .iter()
            .all(|row| row.accounting_key == format!("blob:{}", blob.blob))
    );
    assert_eq!(
        orch_engine::storage::totals(
            &completed
                .iter()
                .map(|row| (**row).clone())
                .collect::<Vec<_>>()
        )
        .bytes,
        blob.size
    );
    let browser = report
        .items
        .iter()
        .find_map(|row| {
            row.upload
                .as_ref()
                .filter(|upload| upload.session_id == Some(partial.upload_id))
        })
        .unwrap();
    assert_eq!(browser.state, UploadInventoryState::Partial);
    let machine_row = report
        .items
        .iter()
        .find_map(|row| {
            row.upload
                .as_ref()
                .filter(|upload| upload.blob == Some(machine_hash))
        })
        .unwrap();
    assert_eq!(machine_row.kind, UploadInventoryKind::Machine);
    assert_eq!(machine_row.machine.as_ref().unwrap().id, machine);
    assert!(
        machine_row.uploader.is_none(),
        "a machine's owner is not a recorded sender"
    );
    assert!(!machine_row.can_open);
    let content = completed[0].upload.as_ref().unwrap().content_id.unwrap();
    let query = UploadBlobGetQuery {
        name: "original.bin".into(),
        ..Default::default()
    };
    places::admin_mode(&world, &admin, false).await;
    let denied = world
        .app
        .call(AdminUploadContentGet, &[&content])
        .token(&admin.token)
        .query(&query)
        .send_bytes()
        .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    places::admin_mode(&world, &admin, true).await;
    let opening = new_id().to_string();
    for _ in 0..2 {
        let opened = world
            .app
            .call(AdminUploadContentGet, &[&content])
            .token(&admin.token)
            .query(&query)
            .header("x-orch-opening", &opening)
            .header("range", "bytes=0-5")
            .send_bytes()
            .await;
        assert_eq!(opened.status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(opened.raw, &bytes[..6]);
        assert_eq!(opened.headers["x-content-type-options"], "nosniff");
    }
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND actor_id=$2 AND action='admin.opened' AND as_admin")
        .bind(content).bind(admin.id()).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(count, 1);
    let missing = world
        .app
        .call(AdminUploadContentGet, &[&new_id()])
        .token(&admin.token)
        .query(&query)
        .send_bytes()
        .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn administrative_previews_use_the_existing_reader_but_api_keys_never_become_browser_openings()
 {
    let world = World::new().await;
    let owner = places::person(&world, "preview-inventory-owner").await;
    let admin = places::admin(&world, "preview-inventory-admin").await;
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(4, 4)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    whole(&world, &owner, "private-picture.png", png.get_ref()).await;
    let own = list(&world, &owner.token, &PageQuery::default()).await;
    let content = own.items[0].content_id.unwrap();
    let query = UploadBlobGetQuery {
        name: "private-picture.png".into(),
        preview: Some(true),
        ..Default::default()
    };
    let api = key(
        &world,
        admin.id(),
        vec![
            json!({"target":{"kind":"system","reach":"self"},"resources":["system"],
        "allow":[{"operation":"AdminUploadContentGet","actions":["read"]}]}),
        ],
    )
    .await;
    let denied = world
        .app
        .call(AdminUploadContentGet, &[&content])
        .token(&api)
        .query(&query)
        .send_bytes()
        .await;
    assert!(!denied.status.is_success());
    let refusal: Json = serde_json::from_slice(&denied.raw).unwrap();
    assert_eq!(refusal["code"], "auth.session_required");
    let opened = world
        .app
        .call(AdminUploadContentGet, &[&content])
        .token(&admin.token)
        .query(&query)
        .send_bytes()
        .await;
    assert_eq!(opened.status, StatusCode::OK);
    let body: Json = serde_json::from_slice(&opened.raw).unwrap();
    assert_eq!(body["state"], "ready");
    assert_eq!(body["width"], 4);
    assert_eq!(body["height"], 4);
    assert_eq!(opened.headers["cache-control"], "private, no-store");
}

use orch_blob_contract::{
    range::ByteRange,
    store::{BlobError, BlobRead, BlobStore, ByteStream, Stored},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::Notify;
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
        let read = self.inner.get(hash, range).await?;
        if *hash == self.hash {
            self.opens.fetch_add(1, Ordering::SeqCst);
            if !self.used.swap(true, Ordering::SeqCst) {
                self.reached.notify_one();
                self.release.notified().await;
            }
        }
        Ok(read)
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
fn gate_open(world: &mut World, hash: Hash) -> Arc<OpenGate> {
    use orch_api_core::{registry::Registry, state::AppState, testing::TestApp};
    let old = world.app.app.clone();
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
    world.app = TestApp::over(state, world.app.clock.clone(), registry);
    gate
}

#[tokio::test]
async fn an_admin_open_rechecks_current_mode_and_the_same_custody_after_the_actual_store_opens() {
    const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
    for change in ["live", "mode", "custody"] {
        let mut world = World::new().await;
        let owner = places::person(&world, "gated-inventory-owner").await;
        let admin = places::admin(&world, "gated-inventory-admin").await;
        let bytes = b"private content must not arrive after authority or custody closes";
        let blob = whole(&world, &owner, "private-body.png", bytes).await;
        let own = list(&world, &owner.token, &PageQuery::default()).await;
        let content = own.items[0].content_id.unwrap();
        let gate = gate_open(&mut world, blob.blob);
        let app = world.app.clone();
        let token = admin.token.clone();
        let waiter = tokio::spawn(async move {
            let request = app
                .call(AdminUploadContentGet, &[&content])
                .token(&token)
                .query(&UploadBlobGetQuery {
                    name: "private-body.bin".into(),
                    ..Default::default()
                });
            request.send_bytes().await
        });
        tokio::time::timeout(WAIT, gate.reached.notified())
            .await
            .unwrap();
        assert_eq!(gate.opens.load(Ordering::SeqCst), 1);
        if change == "mode" {
            places::admin_mode(&world, &admin, false).await;
        } else if change == "custody" {
            world
                .app
                .call(UploadBlobDelete, &[&blob.blob])
                .token(&owner.token)
                .query(&UploadBlobDeleteQuery::default())
                .send()
                .await
                .ok();
        }
        gate.release.notify_one();
        let reply = tokio::time::timeout(WAIT, waiter).await.unwrap().unwrap();
        if change == "live" {
            assert_eq!(
                reply.status,
                StatusCode::OK,
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(reply.raw.as_ref(), bytes.as_slice());
        } else {
            assert!(
                !reply.status.is_success(),
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert!(!reply.raw.windows(bytes.len()).any(|part| part == bytes));
        }
        assert_eq!(
            gate.opens.load(Ordering::SeqCst),
            1,
            "publication must not replay the native opener"
        );
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE action='admin.opened' AND object_id=$1",
        )
        .bind(content)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
        assert_eq!(count, i64::from(change == "live"));
    }
}

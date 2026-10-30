//! Uploaded bytes require their owner's reference or a readable source, and private drafts keep their own files through collection.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod support;
mod workspace;

use chat_world::Room;
use chrono::Duration;
use http::StatusCode;
use orch_api_contract::chats::{
    ChatAttachmentCreate, ChatAttachmentCreateReply, ChatBlobGet, ChatDraftDelete, ChatDraftGet,
    ChatDraftSet, ChatDraftSetBody, ChatMemberRemove, ChatMessageDelete, ChatPart, ForwardCreate,
    ForwardCreateBody, ForwardDestination, ForwardDestinationChat, ForwardSource,
    ForwardSourceChat,
};
use orch_api_contract::common::{ErrorCode, Problem};
use orch_api_contract::documents::{DocumentFileCreate, NodeDocumentCreate};
use orch_api_contract::threads::{
    FILENAME_HEADER, ThreadAttachmentCreate, ThreadDraftSet, ThreadInputCreate, UploadCreate,
    UploadSessionComplete, UploadSessionCreate, UploadSessionCreateBody, UploadSessionPut,
    UploadSessionPutQuery, UploadedBlob,
};
use orch_api_core::cache::collect_garbage;
use orch_api_core::testing::{RawReply, Reply, TestApp};
use orch_api_core::{blobs::Blobs, registry::Registry, state::AppState};
use orch_blob_local::LocalBlobStore;
use orch_db::queries::storage;
use orch_types::{Hash, Id, new_id};
use places::Who;
use serde_json::json;
use std::{path::PathBuf, sync::Arc};
use tempfile::TempDir;

/// An hour past the configured grace makes the collection boundary unambiguous.
const GRACE_MARGIN_HOURS: i64 = 1;

async fn disk_room() -> (Room, TempDir) {
    let mut room = Room::new().await;
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../target"));
    let directory = tempfile::Builder::new()
        .prefix("kfix-upload-")
        .tempdir_in(target)
        .unwrap();
    let store = Arc::new(
        LocalBlobStore::open(directory.path().to_path_buf())
            .await
            .unwrap(),
    );
    let old = &room.world.app.app;
    let blobs = Blobs::new(store, directory.path());
    old.engine.attach_blobs(blobs.store());
    let state = Arc::new(AppState {
        host_ports: old.host_ports.clone(),
        builtin_sources: old.builtin_sources.clone(),
        db: old.db.clone(),
        config: old.config.clone(),
        clock: old.clock.clone(),
        link: old.link.clone(),
        engine: old.engine.clone(),
        realtime: old.realtime.clone(),
        build: old.build,
        blobs,
        cache: old.cache.clone(),
    });
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    room.world.app = TestApp::over(state, room.world.app.clock.clone(), registry);
    (room, directory)
}

fn part(file: &ChatAttachmentCreateReply) -> ChatPart {
    ChatPart::Attachment {
        blob: file.blob,
        name: file.name.clone(),
        size: file.size,
        media_type: file.media_type.clone(),
    }
}

async fn raw_file(room: &Room, by: &Who, bytes: &[u8]) -> ChatAttachmentCreateReply {
    room.world
        .app
        .call(ChatAttachmentCreate, &[&room.chat])
        .token(&by.token)
        .header(FILENAME_HEADER, "notes.txt")
        .bytes(bytes)
        .send()
        .await
        .ok()
}

async fn reference(room: &Room, by: &Who, hash: Hash) -> Reply<ChatAttachmentCreateReply> {
    room.world
        .app
        .call(ChatAttachmentCreate, &[&room.chat])
        .token(&by.token)
        .raw_body(&json!({ "blob": hash, "name": "copy.txt" }))
        .send()
        .await
}

async fn save(room: &Room, by: &Who, parts: Vec<ChatPart>) {
    room.world
        .app
        .call(ChatDraftSet, &[&room.chat])
        .token(&by.token)
        .body(&ChatDraftSetBody {
            parts,
            reply_to: None,
        })
        .send()
        .await
        .ok();
}

async fn discard(room: &Room, by: &Who) {
    room.world
        .app
        .call(ChatDraftDelete, &[&room.chat])
        .token(&by.token)
        .send()
        .await
        .ok();
}

async fn download(room: &Room, by: &Who, hash: Hash) -> RawReply {
    room.world
        .app
        .call(ChatBlobGet, &[&room.chat, &hash])
        .token(&by.token)
        .send_bytes()
        .await
}

async fn collect_after_grace(room: &Room) {
    let app = &room.world.app.app;
    room.world.advance(Duration::hours(
        i64::try_from(app.config.retention.blob_grace_hours).unwrap() + GRACE_MARGIN_HOURS,
    ));
    collect_garbage(&app.db, &app.blobs, app.clock.as_ref(), &app.config)
        .await
        .unwrap();
}

async fn session_upload(room: &Room, by: &Who, bytes: &[u8]) -> UploadedBlob {
    let upload = room
        .world
        .app
        .call(UploadSessionCreate, &[])
        .token(&by.token)
        .body(&UploadSessionCreateBody {
            size: i64::try_from(bytes.len()).unwrap(),
            name: "source.txt".into(),
            media_type: None,
        })
        .send()
        .await
        .ok()
        .upload_id;
    room.world
        .app
        .call(UploadSessionPut, &[&upload])
        .token(&by.token)
        .query(&UploadSessionPutQuery { offset: 0 })
        .bytes(bytes)
        .send()
        .await
        .ok();
    room.world
        .app
        .call(UploadSessionComplete, &[&upload])
        .token(&by.token)
        .send()
        .await
        .ok()
}

fn same_refusal(known: &Problem, missing: &Problem) {
    assert_eq!(known.code, ErrorCode::NotFound);
    assert_eq!(known.status, missing.status);
    assert_eq!(known.code, missing.code);
    assert_eq!(known.detail, missing.detail);
    assert_eq!(known.extra, missing.extra);
}

async fn seed_legacy_draft(room: &Room, by: &Who, parts: &[ChatPart]) {
    orch_db::queries::chats::save_draft(
        &mut room.world.app.app.db.acquire().await.unwrap(),
        by.id(),
        room.chat,
        &json!(parts),
        None,
        room.world.now(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_legacy_private_draft_is_not_proof_of_authority_to_its_named_bytes() {
    let (room, _directory) = disk_room().await;
    let file = raw_file(&room, &room.bea, b"another person's private upload").await;
    let parts = vec![part(&file)];
    seed_legacy_draft(&room, &room.ada, &parts).await;
    sqlx::raw_sql(include_str!("support/kfix_upload_upgrade.sql"))
        .execute(room.world.app.app.db.pool())
        .await
        .unwrap();
    assert_eq!(room.pins(&file.blob).await, 1);
    assert_eq!(
        download(&room, &room.ada, file.blob).await.status,
        StatusCode::NOT_FOUND
    );
    let denied = reference(&room, &room.ada, file.blob).await.problem();
    let missing = reference(&room, &room.ada, Hash::of(b"unuploaded legacy reference"))
        .await
        .problem();
    same_refusal(&denied, &missing);
    let denied_save = room
        .world
        .app
        .call(ChatDraftSet, &[&room.chat])
        .token(&room.ada.token)
        .body(&ChatDraftSetBody {
            parts,
            reply_to: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(denied_save.code, ErrorCode::NotFound);
    assert_eq!(room.pins(&file.blob).await, 1);
    discard(&room, &room.ada).await;
    assert_eq!(room.pins(&file.blob).await, 0);
}

#[tokio::test]
async fn an_authorized_draft_copy_retains_its_authority_when_its_source_is_removed() {
    let (room, _directory) = disk_room().await;
    let source = room
        .group(&room.bea, &[&room.cai], Some("Source"))
        .await
        .base
        .id;
    let bytes = b"a draft copied these authorized bytes";
    let file = raw_file(&room, &room.bea, bytes).await;
    let original = room
        .send(&room.bea, source, vec![part(&file)], &new_id().to_string())
        .await
        .ok()
        .message;
    save(&room, &room.cai, vec![part(&file)]).await;
    room.world
        .app
        .call(ChatMemberRemove, &[&source, &room.cai.id()])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    room.world
        .app
        .call(ChatMessageDelete, &[&source, &original.id])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    assert_eq!(room.pins(&file.blob).await, 1);
    collect_after_grace(&room).await;
    let fetched = download(&room, &room.cai, file.blob).await;
    assert_eq!(fetched.status, StatusCode::OK);
    assert_eq!(fetched.raw.as_ref(), bytes);
    discard(&room, &room.cai).await;
    collect_after_grace(&room).await;
    let later = raw_file(&room, &room.bea, bytes).await;
    assert_eq!(later.blob, file.blob);
    assert_eq!(
        reference(&room, &room.cai, file.blob).await.problem().code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn upgrading_old_drafts_does_not_release_the_pin_of_reuploaded_missing_bytes() {
    let (room, _directory) = disk_room().await;
    let available = raw_file(&room, &room.bea, b"the available old bytes").await;
    let missing_bytes = b"the old bytes were collected";
    let missing = room
        .world
        .app
        .app
        .blobs
        .put_bytes(missing_bytes.to_vec().into())
        .await
        .unwrap();
    assert!(
        room.world
            .app
            .app
            .blobs
            .store()
            .delete(&missing.hash)
            .await
            .unwrap()
    );
    let text = ChatPart::Text {
        text: "keep the unsent text".into(),
    };
    seed_legacy_draft(
        &room,
        &room.ada,
        &[
            text.clone(),
            part(&available),
            ChatPart::Attachment {
                blob: missing.hash,
                name: "gone.txt".into(),
                size: missing.size,
                media_type: "text/plain".into(),
            },
            part(&available),
        ],
    )
    .await;
    sqlx::raw_sql(include_str!("support/kfix_upload_upgrade.sql"))
        .execute(room.world.app.app.db.pool())
        .await
        .unwrap();
    let kept = room
        .world
        .app
        .call(ChatDraftGet, &[&room.chat])
        .token(&room.ada.token)
        .send()
        .await
        .ok()
        .unwrap();
    assert_eq!(kept.parts, vec![text, part(&available), part(&available)]);
    assert_eq!(room.pins(&available.blob).await, 1);
    let restored = raw_file(&room, &room.bea, missing_bytes).await;
    assert_eq!(restored.blob, missing.hash);
    save(&room, &room.bea, vec![part(&available), part(&restored)]).await;
    discard(&room, &room.ada).await;
    assert_eq!(room.pins(&available.blob).await, 1);
    assert_eq!(room.pins(&restored.blob).await, 1);
    collect_after_grace(&room).await;
    let fetched = download(&room, &room.bea, restored.blob).await;
    assert_eq!(fetched.status, StatusCode::OK);
    assert_eq!(fetched.raw.as_ref(), missing_bytes);
}

#[tokio::test]
async fn an_unsent_private_draft_keeps_downloadable_bytes_after_the_blob_grace_period() {
    let (room, _directory) = disk_room().await;
    let bytes = b"only in a draft, still exactly these bytes";
    let file = raw_file(&room, &room.bea, bytes).await;
    save(&room, &room.bea, vec![part(&file)]).await;
    assert_eq!(room.pins(&file.blob).await, 1);
    collect_after_grace(&room).await;
    let reopened = room
        .world
        .app
        .call(ChatDraftGet, &[&room.chat])
        .token(&room.bea.token)
        .send()
        .await
        .ok()
        .unwrap();
    assert_eq!(reopened.parts, vec![part(&file)]);
    let own = download(&room, &room.bea, file.blob).await;
    assert_eq!(own.status, StatusCode::OK);
    assert_eq!(own.raw.as_ref(), bytes);
    assert_eq!(
        download(&room, &room.cai, file.blob).await.status,
        StatusCode::NOT_FOUND
    );
    assert!(
        room.world
            .app
            .call(ChatDraftGet, &[&room.chat])
            .token(&room.cai.token)
            .send()
            .await
            .ok()
            .is_none()
    );
    discard(&room, &room.bea).await;
    assert_eq!(room.pins(&file.blob).await, 0);
    collect_after_grace(&room).await;
    assert!(
        room.world
            .app
            .app
            .blobs
            .store()
            .size(&file.blob)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn replacing_and_discarding_drafts_release_only_their_own_unique_blob_pins() {
    let (room, _directory) = disk_room().await;
    let first = raw_file(&room, &room.bea, b"same bytes").await;
    let duplicate = raw_file(&room, &room.cai, b"same bytes").await;
    assert_eq!(first.blob, duplicate.blob);
    save(&room, &room.bea, vec![part(&first), part(&first)]).await;
    save(&room, &room.cai, vec![part(&duplicate)]).await;
    let message = room
        .send(
            &room.bea,
            room.chat,
            vec![part(&first)],
            &new_id().to_string(),
        )
        .await
        .ok()
        .message;
    assert_eq!(room.pins(&first.blob).await, 3);
    let replacement = raw_file(&room, &room.bea, b"replacement").await;
    save(&room, &room.bea, vec![part(&first), part(&replacement)]).await;
    assert_eq!(
        room.pins(&first.blob).await,
        3,
        "a retained reference keeps its pin"
    );
    save(
        &room,
        &room.bea,
        vec![part(&replacement), part(&replacement)],
    )
    .await;
    save(&room, &room.bea, vec![part(&replacement)]).await;
    assert_eq!(room.pins(&first.blob).await, 2);
    assert_eq!(room.pins(&replacement.blob).await, 1);
    discard(&room, &room.cai).await;
    discard(&room, &room.cai).await;
    assert_eq!(
        room.pins(&first.blob).await,
        1,
        "the sent message still owns its pin"
    );
    discard(&room, &room.bea).await;
    assert_eq!(room.pins(&replacement.blob).await, 0);
    collect_after_grace(&room).await;
    assert!(
        room.world
            .app
            .app
            .blobs
            .store()
            .size(&replacement.blob)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        download(&room, &room.cai, first.blob).await.raw.as_ref(),
        b"same bytes"
    );
    room.world
        .app
        .call(ChatMessageDelete, &[&room.chat, &message.id])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    assert_eq!(room.pins(&first.blob).await, 0);
}

#[tokio::test]
async fn a_rejected_draft_replacement_preserves_the_saved_parts_and_their_pins() {
    let (room, _directory) = disk_room().await;
    let saved = raw_file(&room, &room.bea, b"saved").await;
    let foreign = raw_file(&room, &room.cai, b"another person's upload").await;
    let new = raw_file(&room, &room.bea, b"unused new bytes").await;
    save(&room, &room.bea, vec![part(&saved)]).await;
    let refused = room
        .world
        .app
        .call(ChatDraftSet, &[&room.chat])
        .token(&room.bea.token)
        .body(&ChatDraftSetBody {
            parts: vec![part(&new), part(&foreign)],
            reply_to: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert_eq!(room.pins(&saved.blob).await, 1);
    assert_eq!(room.pins(&new.blob).await, 0);
    assert_eq!(room.pins(&foreign.blob).await, 0);
    let current = room
        .world
        .app
        .call(ChatDraftGet, &[&room.chat])
        .token(&room.bea.token)
        .send()
        .await
        .ok()
        .unwrap();
    assert_eq!(current.parts, vec![part(&saved)]);
}

#[tokio::test]
async fn concurrent_saves_keep_exactly_the_last_saved_drafts_pin() {
    let (room, _directory) = disk_room().await;
    let left = raw_file(&room, &room.bea, b"left").await;
    let right = raw_file(&room, &room.bea, b"right").await;
    tokio::join!(
        save(&room, &room.bea, vec![part(&left)]),
        save(&room, &room.bea, vec![part(&right)])
    );
    let saved = room
        .world
        .app
        .call(ChatDraftGet, &[&room.chat])
        .token(&room.bea.token)
        .send()
        .await
        .ok()
        .unwrap();
    for file in [&left, &right] {
        assert_eq!(
            room.pins(&file.blob).await,
            i32::from(saved.parts == vec![part(file)])
        );
    }
    discard(&room, &room.bea).await;
    assert_eq!(
        room.pins(&left.blob).await + room.pins(&right.blob).await,
        0
    );
}

#[tokio::test]
async fn knowing_a_completed_uploads_hash_does_not_authorize_another_person_to_attach_or_send_it() {
    let (room, _directory) = disk_room().await;
    let bytes = b"upload completion must remember whose bytes these are";
    let uploaded = session_upload(&room, &room.bea, bytes).await;
    let foreign = reference(&room, &room.cai, uploaded.blob).await.problem();
    let missing = reference(&room, &room.cai, Hash::of(b"never uploaded"))
        .await
        .problem();
    same_refusal(&foreign, &missing);
    let file = ChatAttachmentCreateReply {
        blob: uploaded.blob,
        name: uploaded.name,
        size: uploaded.size,
        media_type: uploaded.media_type,
    };
    let direct = room
        .send(
            &room.cai,
            room.chat,
            vec![part(&file)],
            &new_id().to_string(),
        )
        .await
        .problem();
    assert_eq!(direct.code, ErrorCode::NotFound);
    let reused = reference(&room, &room.bea, file.blob).await.ok();
    assert_eq!(reused.blob, file.blob);
    assert_eq!(
        room.pins(&file.blob).await,
        0,
        "an unattached upload remains collectable"
    );
    let independently_uploaded = session_upload(&room, &room.cai, bytes).await;
    assert_eq!(independently_uploaded.blob, file.blob);
    reference(&room, &room.cai, file.blob).await.ok();
    assert_eq!(
        reference(&room, &room.gus, file.blob).await.problem().code,
        ErrorCode::NotFound
    );
    save(&room, &room.cai, vec![part(&file)]).await;
    collect_after_grace(&room).await;
    assert_eq!(
        download(&room, &room.cai, file.blob).await.raw.as_ref(),
        bytes
    );
}

#[tokio::test]
async fn readable_sources_can_be_copied_and_a_revoked_source_cannot_be_used_by_hash() {
    let (room, _directory) = disk_room().await;
    let source = room
        .group(&room.bea, &[&room.cai], Some("Source"))
        .await
        .base
        .id;
    let file = raw_file(&room, &room.bea, b"readable source bytes").await;
    let original = room
        .send(&room.bea, source, vec![part(&file)], &new_id().to_string())
        .await
        .ok()
        .message;
    let forward = |chat: Id| ForwardCreateBody {
        from: ForwardSource::Chat(ForwardSourceChat {
            chat_id: source,
            message_ids: vec![original.id],
        }),
        to: vec![ForwardDestination::Chat(ForwardDestinationChat {
            chat_id: chat,
        })],
        comment: None,
        client_id: new_id().to_string(),
    };
    let unauthorized = room
        .world
        .app
        .call(ForwardCreate, &[])
        .token(&room.gus.token)
        .body(&forward(room.chat))
        .send()
        .await
        .problem();
    assert_eq!(unauthorized.code, ErrorCode::NotFound);
    assert_eq!(
        reference(&room, &room.gus, file.blob).await.problem().code,
        ErrorCode::NotFound
    );
    let copied = reference(&room, &room.cai, file.blob).await.ok();
    let destination = room
        .group(&room.cai, &[&room.gus], Some("Copy"))
        .await
        .base
        .id;
    let forwarded = room
        .world
        .app
        .call(ForwardCreate, &[])
        .token(&room.cai.token)
        .body(&forward(destination))
        .send()
        .await
        .ok();
    assert_eq!(forwarded.chats.len(), 1);
    room.world
        .app
        .call(ChatMemberRemove, &[&source, &room.cai.id()])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    room.world
        .app
        .call(ChatMessageDelete, &[&source, &original.id])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    room.send(
        &room.cai,
        room.chat,
        vec![part(&copied)],
        &new_id().to_string(),
    )
    .await
    .ok();
    let fetched = room
        .world
        .app
        .call(ChatBlobGet, &[&destination, &file.blob])
        .token(&room.gus.token)
        .send_bytes()
        .await;
    assert_eq!(fetched.status, StatusCode::OK);
    assert_eq!(fetched.raw.as_ref(), b"readable source bytes");
    let unseen = raw_file(&room, &room.bea, b"revoked source only").await;
    room.send(
        &room.bea,
        source,
        vec![part(&unseen)],
        &new_id().to_string(),
    )
    .await
    .ok();
    assert_eq!(
        reference(&room, &room.cai, unseen.blob)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn document_attachment_references_require_an_upload_or_a_readable_file_source() {
    let (room, _directory) = disk_room().await;
    let private = room
        .world
        .app
        .call(NodeDocumentCreate, &[&room.topic])
        .token(&room.ada.token)
        .raw_body(&json!({ "title": "Private files", "access": "private" }))
        .send()
        .await
        .ok();
    let public = room
        .world
        .app
        .call(NodeDocumentCreate, &[&room.topic])
        .token(&room.ada.token)
        .raw_body(&json!({ "title": "Readable files" }))
        .send()
        .await
        .ok();
    let file = raw_file(&room, &room.bea, b"private document bytes").await;
    let foreign = room
        .world
        .app
        .call(DocumentFileCreate, &[&private.base.id])
        .token(&room.ada.token)
        .raw_body(&json!({ "blob": file.blob, "name": "notes.txt" }))
        .send()
        .await
        .problem();
    assert_eq!(foreign.code, ErrorCode::NotFound);
    room.world
        .app
        .call(DocumentFileCreate, &[&private.base.id])
        .token(&room.ada.token)
        .header(FILENAME_HEADER, "private.txt")
        .bytes(b"only the private document holds this")
        .send()
        .await
        .ok();
    let private_hash = Hash::of(b"only the private document holds this");
    assert_eq!(
        reference(&room, &room.cai, private_hash)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    room.world
        .app
        .call(DocumentFileCreate, &[&public.base.id])
        .token(&room.ada.token)
        .header(FILENAME_HEADER, "readable.txt")
        .bytes(b"readable document file")
        .send()
        .await
        .ok();
    let readable = reference(&room, &room.cai, Hash::of(b"readable document file"))
        .await
        .ok();
    assert_eq!(
        readable.size,
        i64::try_from(b"readable document file".len()).unwrap()
    );
}

#[tokio::test]
async fn thread_attachments_inputs_and_drafts_cannot_bypass_upload_ownership_with_a_hash() {
    let site = workspace::Site::new().await;
    let bytes = b"another person's whole image upload";
    let uploaded = site
        .app()
        .call(UploadCreate, &[])
        .token(&site.vic.token)
        .header("content-type", "image/png")
        .header(FILENAME_HEADER, "source.png")
        .bytes(bytes)
        .send()
        .await
        .ok();
    let attached = site
        .app()
        .call(ThreadAttachmentCreate, &[&site.thread])
        .token(&site.ada.token)
        .raw_body(&json!({ "blob": uploaded.blob, "name": "copy.png" }))
        .send()
        .await
        .problem();
    assert_eq!(attached.code, ErrorCode::NotFound);
    let content = json!([{ "type": "image", "blob": uploaded.blob, "name": "source.png", "media_type": "image/png" }]);
    let input = site
        .app()
        .call(ThreadInputCreate, &[&site.thread])
        .token(&site.ada.token)
        .raw_body(
            &json!({ "content": content, "delivery": "queue", "client_id": new_id().to_string() }),
        )
        .send()
        .await
        .problem();
    assert_eq!(input.code, ErrorCode::NotFound);
    let draft = site
        .app()
        .call(ThreadDraftSet, &[&site.thread])
        .token(&site.ada.token)
        .raw_body(&json!({ "content": content, "delivery": "queue" }))
        .send()
        .await
        .problem();
    assert_eq!(draft.code, ErrorCode::NotFound);
    let own = site
        .app()
        .call(UploadCreate, &[])
        .token(&site.ada.token)
        .header("content-type", "image/png")
        .header(FILENAME_HEADER, "source.png")
        .bytes(bytes)
        .send()
        .await
        .ok();
    assert_eq!(own.blob, uploaded.blob);
    site.app()
        .call(ThreadAttachmentCreate, &[&site.thread])
        .token(&site.ada.token)
        .raw_body(&json!({ "blob": uploaded.blob, "name": "copy.png" }))
        .send()
        .await
        .ok();
    site.app()
        .call(ThreadDraftSet, &[&site.thread])
        .token(&site.ada.token)
        .raw_body(&json!({ "content": content, "delivery": "queue" }))
        .send()
        .await
        .ok();
    let mut conn = site.world.conn().await;
    assert!(storage::blob(&mut conn, &own.blob).await.unwrap().is_some());
}

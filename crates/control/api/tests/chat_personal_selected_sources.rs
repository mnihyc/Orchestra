//! The ordinary draft writer retains the planner's exact byte source through custody.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod chat_world;
mod places;
mod support;

use chat_world::{Room, text};
use orch_api_contract::chats::{
    ChatAttachmentCreate, ChatDraftGet, ChatDraftSet, ChatDraftSetBody, ChatMessageDelete, ChatPart,
};
use orch_authz::{Caller, Principal};
use orch_chat::chats::{self, Mode};
use orch_chat::following::BlobOrigin;
use orch_chat::{Acting, Error, following};
use orch_types::{Hash, Time, new_id};

async fn pins(room: &Room, hash: Hash) -> i32 {
    sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(hash.as_bytes().as_slice())
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_selected_saved_source_cannot_be_replaced_by_personal_custody_inside_the_draft_writer() {
    let room = Room::new().await;
    let upload = room
        .world
        .app
        .call(ChatAttachmentCreate, &[&room.chat])
        .token(&room.bea.token)
        .header(
            orch_api_contract::threads::FILENAME_HEADER,
            "selected-source.txt",
        )
        .bytes(b"selected saved-source bytes")
        .send()
        .await
        .ok();
    let attachment = ChatPart::Attachment {
        blob: upload.blob,
        name: upload.name,
        size: upload.size,
        media_type: upload.media_type,
    };
    let source = room
        .send(
            &room.bea,
            room.chat,
            vec![attachment.clone()],
            &new_id().to_string(),
        )
        .await
        .ok()
        .message;
    let destination = room.direct(&room.bea, &room.ada).await.base.id;
    let selected = [(upload.blob, BlobOrigin::Chat(room.chat))];
    let audit = orch_db::audit::Origin::default();
    {
        let mut tx = room.world.db.pool.begin().await.unwrap();
        let principal = Principal::load(
            &mut tx,
            Caller::session(room.bea.id()),
            Time::new(room.world.now()),
        )
        .await
        .unwrap();
        let acting = Acting {
            principal: &principal,
            now: room.world.now(),
            origin: &audit,
        };
        assert_eq!(
            following::blob_origin_in(&mut tx, &principal, &upload.blob, &[selected[0].1])
                .await
                .unwrap(),
            Some(selected[0].1)
        );
        let loaded = chats::load(&mut tx, &acting, destination, Mode::Read)
            .await
            .unwrap();
        let saved = following::save_draft_with_blob_origins(
            &mut tx,
            &acting,
            &loaded,
            std::slice::from_ref(&attachment),
            None,
            &selected,
        )
        .await
        .unwrap();
        assert_eq!(saved.parts, vec![attachment.clone()]);
        tx.commit().await.unwrap();
    }
    let before = room
        .world
        .app
        .call(ChatDraftGet, &[&destination])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    let before_pins = pins(&room, upload.blob).await;
    room.world
        .app
        .call(ChatMessageDelete, &[&room.chat, &source.id])
        .token(&room.bea.token)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    let changed = vec![
        text("This replacement must not be saved."),
        attachment.clone(),
    ];
    {
        let mut tx = room.world.db.pool.begin().await.unwrap();
        let principal = Principal::load(
            &mut tx,
            Caller::session(room.bea.id()),
            Time::new(room.world.now()),
        )
        .await
        .unwrap();
        let acting = Acting {
            principal: &principal,
            now: room.world.now(),
            origin: &audit,
        };
        assert_eq!(
            following::blob_origin(&mut tx, &principal, &upload.blob)
                .await
                .unwrap(),
            Some(BlobOrigin::RetainedByCaller),
            "ordinary personal custody remains a real alternate source"
        );
        assert!(
            following::blob_origin_in(&mut tx, &principal, &upload.blob, &[selected[0].1])
                .await
                .unwrap()
                .is_none(),
            "the originally selected saved message no longer supplies these bytes"
        );
        let loaded = chats::load(&mut tx, &acting, destination, Mode::Read)
            .await
            .unwrap();
        let result = following::save_draft_with_blob_origins(
            &mut tx, &acting, &loaded, &changed, None, &selected,
        )
        .await;
        assert!(
            matches!(result, Err(Error::NotFound)),
            "an admitted origin cannot silently fall back to custody"
        );
        tx.rollback().await.unwrap();
    }
    assert_eq!(
        pins(&room, upload.blob).await,
        before_pins - 1,
        "only the ordinary source deletion removed a pin; the refused draft changed none"
    );
    assert_eq!(
        room.world
            .app
            .call(ChatDraftGet, &[&destination])
            .token(&room.bea.token)
            .send()
            .await
            .ok(),
        before
    );
    // This is a restriction on retained selected witnesses, not a new ordinary refusal.
    let native = room
        .world
        .app
        .call(ChatDraftSet, &[&destination])
        .token(&room.bea.token)
        .body(&ChatDraftSetBody {
            parts: changed.clone(),
            reply_to: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(native.parts, changed);
}

//! Selected blob origins remain associated with the bytes and ordinarily readable after planning.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod support;

use chat_world::Room;
use orch_api_contract::chats::{ChatGet, ChatMessageDelete, ChatPart};
use orch_authz::{Caller, Principal};
use orch_chat::following::{BlobOrigin, blob_origin, blob_origin_in, blob_origins};
use orch_types::{Hash, Time, new_id};

fn file(blob: Hash, size: i64) -> ChatPart {
    ChatPart::Attachment {
        blob,
        name: "source.txt".into(),
        size,
        media_type: "text/plain".into(),
    }
}

#[tokio::test]
async fn a_disallowed_retained_origin_can_fall_back_to_a_current_readable_saved_source() {
    let room = Room::new().await;
    let source = room
        .group(&room.bea, &[&room.cai], Some("Saved source"))
        .await
        .base
        .id;
    let (hash, size) = room
        .store(b"Retained by the uploader and also saved in a chat")
        .await;
    let message = room
        .send(
            &room.bea,
            source,
            vec![file(hash, size)],
            &new_id().to_string(),
        )
        .await
        .ok()
        .message;
    let mut conn = room.world.conn().await;
    let caller = Principal::load(
        &mut conn,
        Caller::session(room.bea.id()),
        Time::new(room.world.now()),
    )
    .await
    .unwrap();
    let candidates = blob_origins(&mut conn, room.bea.id(), &hash).await.unwrap();
    assert_eq!(candidates.first(), Some(&BlobOrigin::RetainedByCaller));
    assert!(candidates.contains(&BlobOrigin::Chat(source)));
    assert_eq!(
        blob_origin(&mut conn, &caller, &hash).await.unwrap(),
        Some(BlobOrigin::RetainedByCaller)
    );
    assert_eq!(
        blob_origin_in(&mut conn, &caller, &hash, &[BlobOrigin::Chat(source)])
            .await
            .unwrap(),
        Some(BlobOrigin::Chat(source)),
        "a planner cut on personal retention cannot mask another allowed readable origin"
    );
    assert_eq!(
        blob_origin_in(&mut conn, &caller, &hash, &[])
            .await
            .unwrap(),
        None
    );
    drop(conn);
    room.world
        .app
        .call(ChatMessageDelete, &[&source, &message.id])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    let mut conn = room.world.conn().await;
    let caller = Principal::load(
        &mut conn,
        Caller::session(room.bea.id()),
        Time::new(room.world.now()),
    )
    .await
    .unwrap();
    assert_eq!(
        blob_origin_in(&mut conn, &caller, &hash, &[BlobOrigin::Chat(source)])
            .await
            .unwrap(),
        None,
        "the earlier candidate identity is not a retained proof of hash/source association"
    );
    assert_eq!(
        blob_origin_in(&mut conn, &caller, &hash, &[BlobOrigin::RetainedByCaller])
            .await
            .unwrap(),
        Some(BlobOrigin::RetainedByCaller),
        "existing personal retention does not recheck its former source"
    );
    assert_eq!(
        blob_origin_in(
            &mut conn,
            &caller,
            &Hash::of(b"different bytes"),
            &candidates
        )
        .await
        .unwrap(),
        None,
        "identities obtained for one hash cannot authorize another hash"
    );
}

#[tokio::test]
async fn an_allowed_candidate_list_does_not_grant_ordinary_access_or_fabricate_an_association() {
    let room = Room::new().await;
    let hidden = room
        .group(&room.bea, &[&room.gus], Some("Unreadable source"))
        .await
        .base
        .id;
    let visible = room
        .group(&room.bea, &[&room.cai], Some("Readable source"))
        .await
        .base
        .id;
    let unrelated = room
        .group(&room.bea, &[&room.cai], Some("No matching bytes"))
        .await
        .base
        .id;
    let (hash, size) = room.store(b"One hash in two distinct saved sources").await;
    for chat in [hidden, visible] {
        room.send(
            &room.bea,
            chat,
            vec![file(hash, size)],
            &new_id().to_string(),
        )
        .await
        .ok();
    }
    assert!(
        !room
            .world
            .app
            .call(ChatGet, &[&hidden])
            .token(&room.cai.token)
            .send()
            .await
            .status
            .is_success()
    );
    assert_eq!(room.chat(&room.cai, visible).await.base.id, visible);
    assert_eq!(room.chat(&room.cai, unrelated).await.base.id, unrelated);
    let mut conn = room.world.conn().await;
    let caller = Principal::load(
        &mut conn,
        Caller::session(room.cai.id()),
        Time::new(room.world.now()),
    )
    .await
    .unwrap();
    let candidates = blob_origins(&mut conn, room.cai.id(), &hash).await.unwrap();
    assert!(!candidates.contains(&BlobOrigin::RetainedByCaller));
    assert!(candidates.contains(&BlobOrigin::Chat(hidden)));
    assert!(candidates.contains(&BlobOrigin::Chat(visible)));
    assert!(!candidates.contains(&BlobOrigin::Chat(unrelated)));
    assert_eq!(
        blob_origin_in(&mut conn, &caller, &hash, &[BlobOrigin::Chat(hidden)])
            .await
            .unwrap(),
        None,
        "the planner's allowed identity is not ordinary Read authority"
    );
    assert_eq!(
        blob_origin_in(
            &mut conn,
            &caller,
            &hash,
            &[BlobOrigin::Chat(hidden), BlobOrigin::Chat(visible)]
        )
        .await
        .unwrap(),
        Some(BlobOrigin::Chat(visible)),
        "an unreadable candidate does not suppress another readable allowed source"
    );
    assert_eq!(
        blob_origin(&mut conn, &caller, &hash).await.unwrap(),
        Some(BlobOrigin::Chat(visible))
    );
    assert_eq!(
        blob_origin_in(&mut conn, &caller, &hash, &[BlobOrigin::Chat(unrelated)])
            .await
            .unwrap(),
        None,
        "a readable resource must really contain these bytes"
    );
    assert_eq!(
        blob_origin_in(&mut conn, &caller, &hash, &[BlobOrigin::RetainedByCaller])
            .await
            .unwrap(),
        None,
        "another person's retained upload does not become this caller's custody"
    );
}

#[tokio::test]
async fn a_chat_writer_rechecks_its_selected_origin_and_never_falls_back_to_unbound_custody() {
    use orch_chat::chats::{Mode, load};
    use orch_chat::messages::{Post, post_with_blob_origins};
    use orch_chat::redaction::Redactor;
    let room = Room::new().await;
    let source = room
        .group(&room.bea, &[&room.cai], Some("Selected source"))
        .await
        .base
        .id;
    let destination = room
        .group(&room.bea, &[&room.cai], Some("Destination"))
        .await
        .base
        .id;
    let (hash, size) = room
        .store(b"The source chosen by the operation is held through its write")
        .await;
    let original = room
        .send(
            &room.bea,
            source,
            vec![file(hash, size)],
            &new_id().to_string(),
        )
        .await
        .ok()
        .message;
    let selected = [(hash, BlobOrigin::Chat(source))];
    // First prove the selected origin and the unchanged ordinary writer are both usable.
    {
        let mut tx = room.world.db.pool.begin().await.unwrap();
        let principal = Principal::load(
            &mut tx,
            Caller::session(room.bea.id()),
            Time::new(room.world.now()),
        )
        .await
        .unwrap();
        let origin = orch_db::audit::Origin::default();
        let act = orch_chat::Acting {
            principal: &principal,
            now: room.world.now(),
            origin: &origin,
        };
        let mut loaded = load(&mut tx, &act, destination, Mode::Read).await.unwrap();
        let written = post_with_blob_origins(
            &mut tx,
            &act,
            &mut loaded,
            Post {
                parts: vec![file(hash, size)],
                reply_to: None,
                client_id: new_id().to_string(),
            },
            &Redactor::none(),
            &selected,
        )
        .await
        .unwrap();
        assert!(written.created);
        tx.commit().await.unwrap();
    }
    room.world
        .app
        .call(ChatMessageDelete, &[&source, &original.id])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM chat_messages WHERE chat_id=$1")
        .bind(destination)
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap();
    {
        let mut tx = room.world.db.pool.begin().await.unwrap();
        let principal = Principal::load(
            &mut tx,
            Caller::session(room.bea.id()),
            Time::new(room.world.now()),
        )
        .await
        .unwrap();
        assert_eq!(
            blob_origin(&mut tx, &principal, &hash).await.unwrap(),
            Some(BlobOrigin::RetainedByCaller),
            "the same account still has valid personal custody and another saved source"
        );
        let origin = orch_db::audit::Origin::default();
        let act = orch_chat::Acting {
            principal: &principal,
            now: room.world.now(),
            origin: &origin,
        };
        let mut loaded = load(&mut tx, &act, destination, Mode::Read).await.unwrap();
        let refused = post_with_blob_origins(
            &mut tx,
            &act,
            &mut loaded,
            Post {
                parts: vec![file(hash, size)],
                reply_to: None,
                client_id: new_id().to_string(),
            },
            &Redactor::none(),
            &selected,
        )
        .await;
        assert!(
            matches!(refused, Err(orch_chat::Error::NotFound)),
            "{refused:?}"
        );
        tx.rollback().await.unwrap();
    }
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM chat_messages WHERE chat_id=$1")
        .bind(destination)
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap();
    assert_eq!(after, before, "a stale selected origin commits no message");
    room.send(
        &room.bea,
        destination,
        vec![file(hash, size)],
        &new_id().to_string(),
    )
    .await
    .ok();
    let final_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chat_messages WHERE chat_id=$1")
            .bind(destination)
            .fetch_one(&room.world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        final_count,
        before + 1,
        "ordinary personal-upload custody remains unchanged"
    );
}

// These two controls use real operation keys and the HTTP operation planner, unlike the direct
// candidate/writer controls above. The planner, not this fixture, chooses the authorized source.
async fn upload_operation_key(room: &Room, rules: serde_json::Value) -> String {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
    let scope: KeyScope = serde_json::from_value(serde_json::json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let stored = serde_json::to_value(scope).unwrap();
    let secret = new_token(API_TOKEN_PREFIX);
    let token_hash = token_hash(&secret);
    let now = room.world.now();
    let mut conn = room.world.conn().await;
    create_api_token_scoped(
        &mut conn,
        &NewApiToken {
            id: new_id(),
            user_id: room.bea.id(),
            name: "Blob operation control",
            token_hash: &token_hash,
            scopes: &[],
            node_id: None,
            at: now,
            // Fixed-clock request checks need only this short ordinary lifetime.
            expires_at: Some(now + chrono::Duration::hours(1)),
        },
        Some(&stored),
    )
    .await
    .unwrap();
    secret
}

fn chat_rule(chat: orch_types::Id, actions: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "target":{"kind":"chat","id":chat,"reach":"self"},
        "allow":[{"operation":"ChatMessageCreate","actions":actions}]
    })
}

fn upload_rule(account: orch_types::Id) -> serde_json::Value {
    serde_json::json!({
        "target":{"kind":"account","id":account,"reach":"self"},
        "resources":["upload"],
        "allow":[{"operation":"ChatMessageCreate","actions":["read"]}]
    })
}

async fn chat_messages(room: &Room, chat: orch_types::Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM chat_messages WHERE chat_id=$1")
        .bind(chat)
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap()
}

fn upload_post(hash: Hash, size: i64) -> orch_api_contract::chats::ChatMessageCreateBody {
    orch_api_contract::chats::ChatMessageCreateBody {
        parts: vec![file(hash, size)],
        reply_to: None,
        agent_review: None,
        client_id: new_id().to_string(),
    }
}

fn receipt_message_id(
    reply: orch_api_core::testing::Reply<orch_api_contract::chats::ChatMessageCreateResponse>,
    chat: orch_types::Id,
) -> orch_types::Id {
    use orch_api_contract::chats::ChatMessageCreateResponse;
    assert_eq!(reply.status, 201, "{:?}", reply.body);
    let raw: serde_json::Value = serde_json::from_slice(&reply.raw).unwrap();
    let ChatMessageCreateResponse::Receipt(receipt) = reply.ok() else {
        panic!("source Read does not grant response-content Read on the destination chat");
    };
    assert_eq!(receipt.chat_id, chat);
    assert_eq!(
        raw,
        serde_json::json!({"projection":"receipt","chat_id":chat,"message_id":receipt.message_id,"seq":receipt.seq})
    );
    receipt.message_id
}

async fn actual_attachment(
    room: &Room,
    message: orch_types::Id,
    chat: orch_types::Id,
    hash: Hash,
    size: i64,
) {
    let (actual_chat, author, content): (orch_types::Id, orch_types::Id, serde_json::Value) =
        sqlx::query_as("SELECT chat_id,author_id,content FROM chat_messages WHERE id=$1")
            .bind(message)
            .fetch_one(&room.world.db.pool)
            .await
            .unwrap();
    assert_eq!((actual_chat, author), (chat, room.bea.id()));
    assert_eq!(content, serde_json::json!([file(hash, size)]));
}

#[tokio::test]
async fn a_real_key_needs_upload_read_at_its_actual_account_to_post_retained_bytes() {
    use orch_api_contract::chats::ChatMessageCreate;
    use orch_api_contract::common::ErrorCode;
    let room = Room::new().await;
    let destination = room
        .group(&room.bea, &[&room.cai], Some("Upload destination"))
        .await
        .base
        .id;
    assert!(room.chat(&room.bea, destination).await.can.send);
    let (hash, size) = room
        .store(b"Bytes personally retained by the invoking account")
        .await;
    let body = upload_post(hash, size);
    let before = chat_messages(&room, destination).await;
    let pins_before = room.pins(&hash).await;
    let write_only = upload_operation_key(
        &room,
        serde_json::json!([chat_rule(destination, &["write"])]),
    )
    .await;
    let refused = room
        .world
        .app
        .call(ChatMessageCreate, &[&destination])
        .token(&write_only)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        ErrorCode::NotFound,
        "the existing unavailable-source concealment is retained"
    );
    assert_eq!(chat_messages(&room, destination).await, before);
    assert_eq!(room.pins(&hash).await, pins_before);
    let wrong_account = upload_operation_key(
        &room,
        serde_json::json!([
            chat_rule(destination, &["write"]),
            upload_rule(room.cai.id())
        ]),
    )
    .await;
    assert_eq!(
        room.world
            .app
            .call(ChatMessageCreate, &[&destination])
            .token(&wrong_account)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        chat_messages(&room, destination).await,
        before,
        "another account target grants no upload custody"
    );
    let permitted = upload_operation_key(
        &room,
        serde_json::json!([
            chat_rule(destination, &["write"]),
            upload_rule(room.bea.id())
        ]),
    )
    .await;
    let message = receipt_message_id(
        room.world
            .app
            .call(ChatMessageCreate, &[&destination])
            .token(&permitted)
            .body(&body)
            .send()
            .await,
        destination,
    );
    actual_attachment(&room, message, destination, hash, size).await;
    assert_eq!(chat_messages(&room, destination).await, before + 1);
    assert_eq!(room.pins(&hash).await, pins_before + 1);
    // The same person's ordinary session has unchanged authority and receives its full response.
    let ordinary = room
        .send(
            &room.bea,
            destination,
            vec![file(hash, size)],
            &new_id().to_string(),
        )
        .await
        .ok();
    assert_eq!(ordinary.message.parts, vec![file(hash, size)]);
}

#[tokio::test]
async fn a_real_key_can_use_a_saved_chat_source_when_its_retained_account_path_is_out_of_scope() {
    use orch_api_contract::chats::ChatMessageCreate;
    use orch_api_contract::common::ErrorCode;
    let room = Room::new().await;
    let source = room
        .group(&room.bea, &[&room.cai], Some("Allowed saved source"))
        .await
        .base
        .id;
    let destination = room
        .group(&room.bea, &[&room.cai], Some("Other destination"))
        .await
        .base
        .id;
    assert_ne!(source, destination);
    let (hash, size) = room
        .store(b"A retained upload also available from a scoped saved chat")
        .await;
    room.send(
        &room.bea,
        source,
        vec![file(hash, size)],
        &new_id().to_string(),
    )
    .await
    .ok();
    assert!(room.chat(&room.bea, source).await.can.send);
    assert!(room.chat(&room.bea, destination).await.can.send);
    {
        let mut conn = room.world.conn().await;
        assert!(
            orch_db::queries::uploads::owns(&mut conn, room.bea.id(), &hash)
                .await
                .unwrap(),
            "the out-of-scope retained path really exists beside the saved source"
        );
    }
    let body = upload_post(hash, size);
    let before = chat_messages(&room, destination).await;
    let pins_before = room.pins(&hash).await;
    let source_before = chat_messages(&room, source).await;
    let no_source = upload_operation_key(
        &room,
        serde_json::json!([chat_rule(destination, &["write"])]),
    )
    .await;
    assert_eq!(
        room.world
            .app
            .call(ChatMessageCreate, &[&destination])
            .token(&no_source)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(chat_messages(&room, destination).await, before);
    assert_eq!(room.pins(&hash).await, pins_before);
    // No Account/Upload grant exists. Source Read stays bound to the different source chat.
    let fallback = upload_operation_key(
        &room,
        serde_json::json!([
            chat_rule(destination, &["write"]),
            chat_rule(source, &["read"])
        ]),
    )
    .await;
    let message = receipt_message_id(
        room.world
            .app
            .call(ChatMessageCreate, &[&destination])
            .token(&fallback)
            .body(&body)
            .send()
            .await,
        destination,
    );
    actual_attachment(&room, message, destination, hash, size).await;
    assert_eq!(chat_messages(&room, destination).await, before + 1);
    assert_eq!(room.pins(&hash).await, pins_before + 1);
    assert_eq!(chat_messages(&room, source).await, source_before);
    let ordinary = room
        .send(
            &room.bea,
            destination,
            vec![file(hash, size)],
            &new_id().to_string(),
        )
        .await
        .ok();
    assert_eq!(ordinary.message.parts, vec![file(hash, size)]);
}

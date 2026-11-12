//! A known group may be asked about without already being a member or viewer.
use super::*;
use chat_world::Room;
use orch_api_contract::chats::{ChatGet, ChatJoinRequest, ChatJoinRequestBody};

const GROUP: &str = "Private group request source";
async fn join(room: &Room, token: &str, chat: Id) -> RawReply {
    room.world
        .app
        .call(ChatJoinRequest, &[&chat])
        .token(token)
        .body(&ChatJoinRequestBody {
            label: Some("literal supplied join label".into()),
            reason: REASON.into(),
            expires_at: None,
        })
        .send_bytes()
        .await
}
async fn join_key(room: &Room, token: &str, chat: Id, header: &str, reason: &str) -> RawReply {
    room.world
        .app
        .call(ChatJoinRequest, &[&chat])
        .token(token)
        .header("Idempotency-Key", header)
        .body(&ChatJoinRequestBody {
            label: Some("literal supplied join label".into()),
            reason: reason.into(),
            expires_at: None,
        })
        .send_bytes()
        .await
}
async fn group(room: &Room) -> Id {
    let id = room
        .group(&room.bea, &[&room.cai], Some(GROUP))
        .await
        .base
        .id;
    let denied = room
        .world
        .app
        .call(ChatGet, &[&id])
        .token(&room.gus.token)
        .send_bytes()
        .await;
    checked(denied, StatusCode::NOT_FOUND);
    id
}

#[tokio::test]
async fn modern_join_preserves_native_known_private_group_without_a_chat_view_prerequisite() {
    let room = Room::new().await;
    let chat = group(&room).await;
    let native = checked(
        join(&room, &room.gus.token, chat).await,
        StatusCode::CREATED,
    );
    assert_eq!(native["approvals"][0]["request"]["membership"], true);
    assert!(!native.to_string().contains(GROUP));
    cancel(&room.world, &room.gus.token, &native["approvals"]).await;
    let old = legacy(&room.world, room.gus.id()).await;
    let made = checked(join(&room, &old.secret, chat).await, StatusCode::CREATED);
    cancel(&room.world, &room.gus.token, &made["approvals"]).await;
    let key = key(
        &room.world,
        room.gus.id(),
        vec![full(room.root, JOIN, &["read", "write", "manage"])],
    )
    .await;
    let made = checked(join(&room, &key.secret, chat).await, StatusCode::CREATED);
    assert!(made.get("projection").is_none());
    assert_eq!(made["approvals"][0]["request"]["membership"], true);
    assert_eq!(made["approvals"][0]["request"]["reason"], REASON);
    assert!(!made.to_string().contains(GROUP));
    let members: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chat_members WHERE chat_id=$1 AND user_id=$2")
            .bind(chat)
            .bind(room.gus.id())
            .fetch_one(&room.world.db.pool)
            .await
            .unwrap();
    assert_eq!(members, 0, "asking does not grant membership");
}

#[tokio::test]
async fn join_write_only_receipt_keeps_the_real_accepted_request_without_disclosing_a_card() {
    let room = Room::new().await;
    let chat = group(&room).await;
    let key = key(
        &room.world,
        room.gus.id(),
        vec![rule("chat", chat, "chat", JOIN, &["write"])],
    )
    .await;
    let made = checked(join(&room, &key.secret, chat).await, StatusCode::CREATED);
    assert_eq!(made["projection"], "receipt");
    assert!(made.get("approvals").is_none());
    let batch = id(&made["batch_id"]);
    let saved:Vec<Id>=sqlx::query_scalar("SELECT id FROM approvals WHERE batch_id=$1 AND object_kind='chat' AND object_id=$2 AND requester_user_id=$3")
        .bind(batch).bind(chat).bind(room.gus.id()).fetch_all(&room.world.db.pool).await.unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(made["approval_ids"], json!(saved));
    assert!(!made.to_string().contains(GROUP) && !made.to_string().contains(REASON));
}

#[tokio::test]
async fn an_unselected_group_cannot_receive_another_chats_join_operation() {
    let room = Room::new().await;
    let chat = group(&room).await;
    let other = room
        .group(&room.bea, &[&room.cai], Some("Other group"))
        .await
        .base
        .id;
    let key = key(
        &room.world,
        room.gus.id(),
        vec![rule("chat", other, "chat", JOIN, &["write"])],
    )
    .await;
    let before = footprint(&room.world, room.gus.id()).await;
    checked(join(&room, &key.secret, chat).await, StatusCode::FORBIDDEN);
    assert_eq!(footprint(&room.world, room.gus.id()).await, before);
}

#[tokio::test]
async fn a_full_key_does_not_turn_a_direct_chat_into_a_joinable_group() {
    let room = Room::new().await;
    let chat = room.direct(&room.bea, &room.cai).await.base.id;
    let native = checked(
        join(&room, &room.gus.token, chat).await,
        StatusCode::NOT_FOUND,
    );
    let key = key(
        &room.world,
        room.gus.id(),
        vec![full(room.root, JOIN, &["read", "write", "manage"])],
    )
    .await;
    let before = footprint(&room.world, room.gus.id()).await;
    let denied = checked(join(&room, &key.secret, chat).await, StatusCode::NOT_FOUND);
    assert_eq!(denied["code"], native["code"]);
    assert_eq!(footprint(&room.world, room.gus.id()).await, before);
}

#[tokio::test]
async fn the_requesters_revoked_key_does_not_undo_a_later_independent_membership_grant() {
    let room = Room::new().await;
    let chat = group(&room).await;
    let key = key(
        &room.world,
        room.gus.id(),
        vec![full(room.root, JOIN, &["read", "write", "manage"])],
    )
    .await;
    let made = checked(join(&room, &key.secret, chat).await, StatusCode::CREATED);
    let approval = id(&made["approvals"][0]["id"]);
    revoke(&room.world, room.gus.id(), &key).await;
    checked(
        room.world
            .app
            .call(ApprovalDecide, &[&approval])
            .token(&room.bea.token)
            .raw_body(&json!({"approve":true}))
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    let joined = room
        .world
        .app
        .call(ChatGet, &[&chat])
        .token(&room.gus.token)
        .send_bytes()
        .await;
    checked(joined, StatusCode::OK);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chat_members WHERE chat_id=$1 AND user_id=$2 AND left_at IS NULL",
    )
    .bind(chat)
    .bind(room.gus.id())
    .fetch_one(&room.world.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn join_creation_keys_read_the_current_question_without_repeating_approval_or_notifications()
{
    let room = Room::new().await;
    let chat = group(&room).await;
    let writer = key(
        &room.world,
        room.gus.id(),
        vec![rule("chat", chat, "chat", JOIN, &["write"])],
    )
    .await;
    let header = new_id().to_string();
    let first = checked(
        join_key(&room, &writer.secret, chat, &header, REASON).await,
        StatusCode::CREATED,
    );
    assert_eq!(first["projection"], "receipt");
    let batch = id(&first["batch_id"]);
    let approval = id(&first["approval_ids"][0]);
    let accepted = footprint(&room.world, room.gus.id()).await;
    checked(
        room.world
            .app
            .call(ApprovalCancel, &[&approval])
            .token(&room.gus.token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    for own in [true, false] {
        let reader = key(
            &room.world,
            room.gus.id(),
            vec![
                rule(
                    "chat",
                    chat,
                    "approval",
                    if own { JOIN } else { GET },
                    &["read"],
                ),
                rule(
                    "org",
                    room.root,
                    "org",
                    if own { JOIN } else { GET },
                    &["read"],
                ),
            ],
        )
        .await;
        // A completed creation retains its first status while reading current cards.
        let replay = checked(
            join_key(&room, &reader.secret, chat, &header, REASON).await,
            StatusCode::CREATED,
        );
        if own {
            assert_eq!(replay["batch_id"], json!(batch));
            assert_eq!(replay["approvals"][0]["id"], json!(approval));
            assert_eq!(replay["approvals"][0]["state"], "cancelled");
            assert_eq!(replay["approvals"][0]["request"]["reason"], REASON);
            checked(
                join_key(&room, &reader.secret, chat, &new_id().to_string(), REASON).await,
                StatusCode::FORBIDDEN,
            );
        } else {
            assert_eq!(replay, first);
        }
        assert_eq!(footprint(&room.world, room.gus.id()).await, accepted);
    }
    let ordinary = checked(
        join_key(&room, &room.gus.token, chat, &header, REASON).await,
        StatusCode::CREATED,
    );
    assert_eq!(ordinary["approvals"][0]["state"], "cancelled");
    checked(
        join_key(
            &room,
            &room.gus.token,
            chat,
            &header,
            "A different question",
        )
        .await,
        StatusCode::CONFLICT,
    );
    assert_eq!(footprint(&room.world, room.gus.id()).await, accepted);
    let members: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chat_members WHERE chat_id=$1 AND user_id=$2 AND left_at IS NULL",
    )
    .bind(chat)
    .bind(room.gus.id())
    .fetch_one(&room.world.db.pool)
    .await
    .unwrap();
    assert_eq!(members, 0, "replaying a question never grants membership");
}

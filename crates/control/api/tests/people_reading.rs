//! Thread chat defaults remain current, and an observed read cannot consume a later deliberate unread choice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod support;

use chat_world::{Room, chat_of};
use orch_api_contract::chats::{
    ChatDefaultReason, ChatLevel, ChatRead, ChatReadBody, ChatStateSet, ChatStateSetBody,
};
use orch_api_contract::common::{ErrorCode, SubscriptionLevel};
use orch_api_contract::nodes::{
    NodeSubscriptionDelete, NodeSubscriptionSet, NodeSubscriptionSetBody,
};
use orch_api_contract::personal::MeChatStateList;
use orch_types::new_id;

async fn thread(room: &Room) -> (orch_types::Id, orch_types::Id) {
    let thread = new_id();
    let mut conn = room.world.conn().await;
    orch_db::queries::hierarchy::testing::insert_node(
        &mut conn,
        thread,
        "thread",
        Some(room.topic),
        "Work",
        room.ada.id(),
    )
    .await
    .unwrap();
    orch_db::queries::access::testing::name_owner(
        &mut conn,
        thread,
        ("user", room.ada.id()),
        room.world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    (thread, chat_of(&room.world, &room.ada, thread).await)
}

#[tokio::test]
async fn a_threads_owner_and_whole_follower_follow_its_chat_without_engagement_and_the_chat_bell_wins()
 {
    let room = Room::new().await;
    let (thread, chat) = thread(&room).await;
    let owned = room.chat(&room.ada, chat).await.my.unwrap();
    assert!(!owned.engaged);
    assert_eq!(owned.effective_level, ChatLevel::All);
    assert_eq!(owned.default_reason, Some(ChatDefaultReason::Owner));
    assert!(room.chat(&room.cai, chat).await.my.is_none());
    room.world
        .app
        .call(NodeSubscriptionSet, &[&thread])
        .token(&room.cai.token)
        .body(&NodeSubscriptionSetBody {
            level: SubscriptionLevel::All,
        })
        .send()
        .await
        .ok();
    let followed = room.chat(&room.cai, chat).await.my.unwrap();
    assert!(!followed.engaged);
    assert_eq!(followed.effective_level, ChatLevel::All);
    assert_eq!(followed.default_reason, Some(ChatDefaultReason::Following));
    let states = room
        .world
        .app
        .call(MeChatStateList, &[])
        .token(&room.cai.token)
        .send()
        .await
        .ok();
    assert!(
        states
            .iter()
            .any(|state| state.chat_id == chat && state.effective_level == ChatLevel::All)
    );
    room.say(&room.bea, chat, "A colleague said something")
        .await;
    let notices: Vec<orch_types::Id> = sqlx::query_scalar(
        "SELECT user_id FROM notifications WHERE chat_id=$1 AND kind='chat.message'",
    )
    .bind(chat)
    .fetch_all(&room.world.db.pool)
    .await
    .unwrap();
    assert!(notices.contains(&room.ada.id()) && notices.contains(&room.cai.id()));
    assert!(!room.chat(&room.ada, chat).await.my.unwrap().engaged);
    room.world
        .app
        .call(ChatStateSet, &[&chat])
        .token(&room.cai.token)
        .body(&ChatStateSetBody {
            level: Some(Some(ChatLevel::Off)),
            hidden: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        room.chat(&room.cai, chat).await.my.unwrap().effective_level,
        ChatLevel::Off
    );
    room.world
        .app
        .call(NodeSubscriptionDelete, &[&thread])
        .token(&room.cai.token)
        .send()
        .await
        .ok();
    room.world
        .app
        .call(ChatStateSet, &[&chat])
        .token(&room.cai.token)
        .body(&ChatStateSetBody {
            level: Some(None),
            hidden: None,
        })
        .send()
        .await
        .ok();
    // Neither following the thread nor ringing its chat any more, Cai reads it and is shown no state of
    // his own in it, as anybody who only reads a chat is.
    assert!(room.chat(&room.cai, chat).await.my.is_none());
    assert!(
        room.chat(&room.ada, room.chat).await.my.is_none(),
        "owning the topic does not follow its chat"
    );
}

#[tokio::test]
async fn an_old_automatic_read_in_either_order_leaves_the_chosen_unread_anchor_and_its_mentions() {
    for old_read_first in [true, false] {
        let room = Room::new().await;
        room.say(&room.ada, room.chat, "one").await;
        let second = room.say(&room.ada, room.chat, "two @bea").await;
        let third = room.say(&room.ada, room.chat, "three @bea").await;
        let before: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications WHERE chat_id=$1")
            .bind(room.chat)
            .fetch_one(&room.world.db.pool)
            .await
            .unwrap();
        if old_read_first {
            let read = room
                .world
                .app
                .call(ChatRead, &[&room.chat])
                .token(&room.bea.token)
                .body(&ChatReadBody {
                    seq: third.seq_created,
                    read_revision: 0,
                    unread_from: None,
                })
                .send()
                .await;
            assert_eq!(read.status, 204);
        }
        let rewind = room
            .world
            .app
            .call(ChatRead, &[&room.chat])
            .token(&room.bea.token)
            .body(&ChatReadBody {
                seq: second.seq_created,
                read_revision: 0,
                unread_from: Some(true),
            })
            .send()
            .await;
        assert_eq!(rewind.status, 204);
        if !old_read_first {
            let stale = room
                .world
                .app
                .call(ChatRead, &[&room.chat])
                .token(&room.bea.token)
                .body(&ChatReadBody {
                    seq: third.seq_created,
                    read_revision: 0,
                    unread_from: None,
                })
                .send()
                .await
                .problem();
            assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
        }
        let unread = room.chat(&room.bea, room.chat).await.my.unwrap();
        assert_eq!(unread.last_read_seq, second.seq_created - 1);
        assert_eq!(unread.read_revision, 1);
        assert_eq!(unread.mentions_unread, 2);
        let after: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications WHERE chat_id=$1")
            .bind(room.chat)
            .fetch_one(&room.world.db.pool)
            .await
            .unwrap();
        assert_eq!(after, before, "rewinding sends no new notice");
        room.world
            .app
            .call(ChatRead, &[&room.chat])
            .token(&room.bea.token)
            .body(&ChatReadBody {
                seq: third.seq_created,
                read_revision: 1,
                unread_from: None,
            })
            .send()
            .await
            .ok();
        let read = room.chat(&room.bea, room.chat).await.my.unwrap();
        assert_eq!(read.last_read_seq, third.seq_created);
        assert_eq!(read.mentions_unread, 0);
    }
}

#[tokio::test]
async fn an_inspector_has_no_read_state_and_an_unread_anchor_must_be_a_live_message() {
    let room = Room::new().await;
    let direct = room.direct(&room.ada, &room.bea).await;
    let message = room.say(&room.ada, direct.base.id, "private talk").await;
    let inspector = room.chat(&room.dee, direct.base.id).await;
    assert!(inspector.my.is_none());
    let refused = room
        .world
        .app
        .call(ChatRead, &[&direct.base.id])
        .token(&room.dee.token)
        .body(&ChatReadBody {
            seq: message.seq_created,
            read_revision: 0,
            unread_from: Some(true),
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let invalid = room
        .world
        .app
        .call(ChatRead, &[&direct.base.id])
        .token(&room.bea.token)
        .body(&ChatReadBody {
            seq: message.seq_created + 1,
            read_revision: 0,
            unread_from: Some(true),
        })
        .send()
        .await
        .problem();
    assert_eq!(invalid.code, ErrorCode::RequestInvalid);
}

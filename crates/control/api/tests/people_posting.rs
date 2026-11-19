//! Only the current chat manager or a group's creator controls an origin's local stop, and both changes are audited.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod support;

use chat_world::Room;
use orch_api_contract::chats::{ChatAgentPostingSet, ChatAgentPostingSetBody};
use orch_api_contract::common::ErrorCode;
use orch_types::{Id, new_id};

async fn origin(room: &Room) -> Id {
    let id = new_id();
    orch_db::queries::hierarchy::testing::insert_node(
        &mut *room.world.conn().await,
        id,
        "thread",
        Some(room.topic),
        "Writer",
        room.ada.id(),
    )
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn a_node_chat_manager_stops_and_allows_an_origin_and_every_real_change_is_audited_once() {
    let room = Room::new().await;
    let thread = origin(&room).await;
    let refused = room
        .world
        .app
        .call(ChatAgentPostingSet, &[&room.chat, &thread])
        .token(&room.bea.token)
        .body(&ChatAgentPostingSetBody { stopped: true })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    for stopped in [true, true, false] {
        room.world
            .app
            .call(ChatAgentPostingSet, &[&room.chat, &thread])
            .token(&room.ada.token)
            .body(&ChatAgentPostingSetBody { stopped })
            .send()
            .await
            .ok();
    }
    let actions: Vec<String> = sqlx::query_scalar("SELECT action FROM audit_log WHERE object_kind='chat' AND object_id=$1 AND action IN ('chat.agent_posting_stopped','chat.agent_posting_allowed') ORDER BY at,id")
        .bind(room.chat).fetch_all(&room.world.db.pool).await.unwrap();
    assert_eq!(actions.len(), 2);
    assert!(
        actions
            .iter()
            .any(|action| action == "chat.agent_posting_stopped")
    );
    assert!(
        actions
            .iter()
            .any(|action| action == "chat.agent_posting_allowed")
    );
    assert!(
        !orch_db::queries::chats::posting::is_stopped(
            &mut *room.world.conn().await,
            room.chat,
            thread
        )
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn only_a_groups_creator_controls_its_stop_and_a_direct_chat_has_no_such_manager() {
    let room = Room::new().await;
    let thread = origin(&room).await;
    let group = room
        .group(&room.cai, &[&room.bea, &room.ada], Some("Crew"))
        .await;
    let denied = room
        .world
        .app
        .call(ChatAgentPostingSet, &[&group.base.id, &thread])
        .token(&room.ada.token)
        .body(&ChatAgentPostingSetBody { stopped: true })
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    room.world
        .app
        .call(ChatAgentPostingSet, &[&group.base.id, &thread])
        .token(&room.cai.token)
        .body(&ChatAgentPostingSetBody { stopped: true })
        .send()
        .await
        .ok();
    let direct = room.direct(&room.ada, &room.bea).await;
    let denied = room
        .world
        .app
        .call(ChatAgentPostingSet, &[&direct.base.id, &thread])
        .token(&room.ada.token)
        .body(&ChatAgentPostingSetBody { stopped: true })
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
}

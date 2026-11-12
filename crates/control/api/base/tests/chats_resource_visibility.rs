//! Public groups use their actual home's chat reader, private groups keep their private door.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "native chat visibility controls"
)]
mod chat_world;
mod places;
mod support;
use chat_world::Room;
use orch_api_contract::chats::{
    Chat, ChatGet, ChatJoin, ChatUpdate, ChatUpdateBody, ChatVisibility, OrgChatCreate,
    OrgChatCreateBody, OrgChatList, OrgChatListQuery,
};
use orch_api_contract::everything::{EverythingList, EverythingListQuery};
use orch_db::queries::chats;
use orch_types::{Id, new_id};
use places::Who;
use serde_json::{Value, json};

async fn make(room: &Room, visibility: Option<ChatVisibility>) -> Chat {
    room.world
        .app
        .call(OrgChatCreate, &[&room.root])
        .token(&room.bea.token)
        .body(&OrgChatCreateBody {
            user_ids: vec![room.gus.id()],
            name: Some("Release group".into()),
            home_id: Some(room.topic),
            visibility,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
}
async fn join(room: &Room, actor: &Who, chat: Id) -> http::StatusCode {
    room.world
        .app
        .call(ChatJoin, &[&chat])
        .token(&actor.token)
        .send()
        .await
        .status
}
async fn branch(room: &Room, actor: &Who, chat: Id) -> bool {
    room.world
        .app
        .call(EverythingList, &[])
        .token(&actor.token)
        .query(&EverythingListQuery {
            parent: Some(room.topic.to_string()),
            kinds: Some("chat".into()),
            ..Default::default()
        })
        .send()
        .await
        .ok()
        .items
        .iter()
        .any(|row| row.id == chat.to_string())
}
#[tokio::test]
async fn a_home_reader_discovers_and_joins_a_public_group_while_an_outsider_does_not() {
    let room = Room::new().await;
    let group = make(&room, Some(ChatVisibility::Public)).await;
    let chat = group.base.id;
    assert_eq!(group.base.visibility, ChatVisibility::Public);
    assert!(room.chat(&room.cai, chat).await.can.join);
    assert_eq!(room.chat(&room.cai, chat).await.base.id, chat);
    assert!(
        !room.chat(&room.cai, chat).await.can.send,
        "a public reader joins before posting"
    );
    assert!(branch(&room, &room.cai, chat).await);
    let list = room
        .world
        .app
        .call(OrgChatList, &[&room.root])
        .token(&room.cai.token)
        .query(&OrgChatListQuery::default())
        .send()
        .await
        .ok();
    assert!(list.items.iter().any(|row| row.base.id == chat));
    assert_eq!(
        room.world
            .app
            .call(ChatGet, &[&chat])
            .token(&room.eve.token)
            .send()
            .await
            .status,
        404
    );
    assert_eq!(join(&room, &room.eve, chat).await, 404);
    assert_eq!(join(&room, &room.cai, chat).await, 204);
    let member = room.chat(&room.cai, chat).await;
    assert!(member.can.send);
    assert!(!member.can.join);
    assert!(!member.can.manage);
    assert!(
        member
            .members
            .unwrap()
            .iter()
            .any(|member| member.id == room.cai.id())
    );
    let before = member.seq;
    assert_eq!(join(&room, &room.cai, chat).await, 204);
    assert_eq!(
        room.chat(&room.cai, chat).await.seq,
        before,
        "a repeated join consumes no sequence"
    );
}
#[tokio::test]
async fn private_group_names_and_members_stay_hidden_and_visibility_changes_need_a_current_manager()
{
    let room = Room::new().await;
    let group = make(&room, None).await;
    let chat = group.base.id;
    assert_eq!(group.base.visibility, ChatVisibility::Private);
    assert!(!branch(&room, &room.cai, chat).await);
    assert_eq!(
        room.world
            .app
            .call(ChatGet, &[&chat])
            .token(&room.cai.token)
            .send()
            .await
            .status,
        404
    );
    assert_eq!(join(&room, &room.cai, chat).await, 404);
    let body = ChatUpdateBody {
        name: None,
        home_id: None,
        visibility: Some(ChatVisibility::Public),
    };
    assert_eq!(
        room.world
            .app
            .call(ChatUpdate, &[&chat])
            .token(&room.gus.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        orch_api_contract::common::ErrorCode::ChatManagerRequired
    );
    let changed = room
        .world
        .app
        .call(ChatUpdate, &[&chat])
        .token(&room.bea.token)
        .body(&body)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(changed.base.visibility, ChatVisibility::Public);
    assert!(branch(&room, &room.cai, chat).await);
    assert_eq!(join(&room, &room.cai, chat).await, 204);
    let body = ChatUpdateBody {
        visibility: Some(ChatVisibility::Private),
        ..body
    };
    room.world
        .app
        .call(ChatUpdate, &[&chat])
        .token(&room.bea.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert!(
        room.chat(&room.cai, chat).await.can.send,
        "a privacy change keeps admitted members"
    );
    assert!(
        !branch(&room, &room.eve, chat).await,
        "a privacy change never exposes the group to an outsider"
    );
}
async fn key(room: &Room, rule: Value) -> String {
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity;
    let scope: orch_access_contract::key_scope::KeyScope =
        serde_json::from_value(json!({"rules":[rule]})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *room.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: room.cai.id(),
            name: "Public join fixture",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: room.world.now(),
            expires_at: Some(room.world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    secret
}
fn rule(op: &str, kind: &str, id: Id, action: &str) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":["chat"],"allow":[{"operation":op,"actions":[action]}]})
}
#[tokio::test]
async fn self_join_admits_only_its_exact_stored_chat_write_and_still_needs_native_home_view() {
    let room = Room::new().await;
    let chat = make(&room, Some(ChatVisibility::Public)).await.base.id;
    for rule in [
        rule("ChatGet", "chat", chat, "write"),
        rule("ChatJoin", "chat", chat, "read"),
        rule("ChatJoin", "org", chat, "write"),
        rule("ChatJoin", "chat", room.chat, "write"),
    ] {
        let token = key(&room, rule).await;
        assert_eq!(
            room.world
                .app
                .call(ChatJoin, &[&chat])
                .token(&token)
                .send()
                .await
                .status,
            403
        );
        assert!(
            !chats::members(&mut *room.world.conn().await, chat)
                .await
                .unwrap()
                .iter()
                .any(|row| row.user_id == room.cai.id() && row.is_in())
        );
    }
    let exact = key(&room, rule("ChatJoin", "chat", chat, "write")).await;
    room.inherit_chat_area(room.topic, false).await;
    let refused = room
        .world
        .app
        .call(ChatJoin, &[&chat])
        .token(&exact)
        .send()
        .await;
    assert!(refused.status.is_client_error());
    room.inherit_chat_area(room.topic, true).await;
    assert_eq!(
        room.world
            .app
            .call(ChatJoin, &[&chat])
            .token(&exact)
            .send()
            .await
            .status,
        204
    );
    assert!(
        !chats::members(&mut *room.world.conn().await, chat)
            .await
            .unwrap()
            .iter()
            .find(|row| row.user_id == room.cai.id())
            .unwrap()
            .manager
    );
}
#[tokio::test]
async fn self_join_rechecks_privacy_after_the_actual_chat_row_wait() {
    for private in [false, true] {
        let room = Room::new().await;
        let chat = make(&room, Some(ChatVisibility::Public)).await.base.id;
        let mut blocker = room.world.db.pool.begin().await.unwrap();
        chats::lock(&mut blocker, chat).await.unwrap().unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = room.world.app.clone();
        let token = room.cai.token.clone();
        let future = tokio::spawn(async move {
            let call = app.call(ChatJoin, &[&chat]).token(&token);
            call.send().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async {
            loop {assert!(!future.is_finished());let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))").bind(pid).fetch_one(&room.world.db.pool).await.unwrap();if blocked {break;}tokio::task::yield_now().await;}
        }).await.expect("the real group row must serialize join and privacy edits");
        if private {
            chats::set_visibility(&mut blocker, chat, "private")
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        assert_eq!(
            future.await.unwrap().status,
            if private { 404 } else { 204 }
        );
        let joined = chats::members(&mut *room.world.conn().await, chat)
            .await
            .unwrap()
            .iter()
            .any(|row| row.user_id == room.cai.id() && row.is_in());
        assert_eq!(joined, !private);
    }
}

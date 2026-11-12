//! Native chat-list intent, selected windows and current publication under exact key rules.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "native scoped-list assertions"
)]
mod chat_world;
mod places;
mod support;

use chat_world::Room;
use chrono::Duration;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::chats::{
    ChatListScope, ChatStateSet, ChatStateSetBody, OrgChatList, OrgChatListQuery,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity;
use orch_types::{Id, Json, new_id};
use places::Who;
use serde_json::json;

fn rule(operation: &str, action: &str, kind: &str, id: Id, reach: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":[action]}]})
}
async fn key(room: &Room, by: &Who, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *room.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: by.id(),
            name: "Native chat-list fixture",
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
fn query(scope: ChatListScope) -> OrgChatListQuery {
    OrgChatListQuery {
        scope: Some(scope),
        participating: None,
        of: None,
        limit: Some(1),
        cursor: None,
    }
}
async fn list(
    room: &Room,
    token: &str,
    query: &OrgChatListQuery,
) -> orch_api_core::testing::RawReply {
    room.world
        .app
        .call(OrgChatList, &[&room.root])
        .token(token)
        .query(query)
        .send_bytes()
        .await
}
fn body(reply: &orch_api_core::testing::RawReply) -> Json {
    serde_json::from_slice(&reply.raw).unwrap()
}
fn ids(value: &Json) -> Vec<Id> {
    value["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|chat| chat["id"].as_str().unwrap().parse().unwrap())
        .collect()
}

#[tokio::test]
async fn chat_lists_filter_actual_chat_targets_before_the_native_window_and_keep_cursors() {
    let room = Room::new().await;
    let old = room.direct(&room.bea, &room.cai).await.base.id;
    room.say(&room.bea, old, "Allowed older preview").await;
    room.world.advance(Duration::seconds(1));
    let middle = room
        .group(&room.bea, &[&room.cai], Some("Allowed middle"))
        .await
        .base
        .id;
    room.say(&room.bea, middle, "Allowed middle preview").await;
    room.world.advance(Duration::seconds(1));
    let excluded = room.direct(&room.bea, &room.gus).await.base.id;
    room.say(&room.bea, excluded, "Excluded newest preview")
        .await;
    let token = key(
        &room,
        &room.bea,
        vec![
            rule("OrgChatList.member", "read", "chat", old, "self"),
            rule("OrgChatList.member", "read", "chat", middle, "self"),
        ],
    )
    .await;
    let mut window = query(ChatListScope::Mine);
    let first = list(&room, &token, &window).await;
    assert_eq!(first.status, StatusCode::OK);
    let first = body(&first);
    assert_eq!(ids(&first), [middle]);
    assert!(!first.to_string().contains("Excluded newest preview"));
    window.cursor = Some(first["next_cursor"].as_str().unwrap().to_owned());
    let last = list(&room, &token, &window).await;
    assert_eq!(last.status, StatusCode::OK);
    let last = body(&last);
    assert_eq!(ids(&last), [old]);
    assert!(last["next_cursor"].is_null());
    let all = key(
        &room,
        &room.bea,
        vec![rule(
            "OrgChatList.member",
            "read",
            "org",
            room.root,
            "subtree",
        )],
    )
    .await;
    let window = query(ChatListScope::Mine);
    let native = list(&room, &room.bea.token, &window).await;
    let scoped = list(&room, &all, &window).await;
    assert_eq!(scoped.status, StatusCode::OK);
    assert_eq!(body(&scoped), body(&native));
    assert_eq!(ids(&body(&scoped)), [excluded]);
}

#[tokio::test]
async fn inspection_is_the_native_manage_variant_and_scope_does_not_grant_ordinary_inspection() {
    let room = Room::new().await;
    let chat = room.direct(&room.bea, &room.cai).await.base.id;
    room.say(&room.bea, chat, "Native inspection preview").await;
    let mut inspection = query(ChatListScope::Others);
    inspection.of = Some(room.root);
    let manage = key(
        &room,
        &room.ada,
        vec![
            rule("OrgChatList.inspect", "manage", "chat", chat, "self"),
            rule("OrgChatList.inspect", "read", "org", room.root, "self"),
        ],
    )
    .await;
    let scoped = list(&room, &manage, &inspection).await;
    assert_eq!(scoped.status, StatusCode::OK);
    assert_eq!(ids(&body(&scoped)), [chat]);
    let native = list(&room, &room.ada.token, &inspection).await;
    assert_eq!(body(&scoped), body(&native));
    for rules in [
        vec![rule("OrgChatList.member", "read", "chat", chat, "self")],
        vec![rule("OrgChatList.inspect", "read", "chat", chat, "self")],
        vec![rule("ChatGet", "read", "chat", chat, "self")],
    ] {
        let token = key(&room, &room.ada, rules).await;
        assert_eq!(
            list(&room, &token, &inspection).await.status,
            StatusCode::FORBIDDEN
        );
    }
    let no_native_access = key(
        &room,
        &room.bea,
        vec![rule(
            "OrgChatList.inspect",
            "manage",
            "org",
            room.root,
            "subtree",
        )],
    )
    .await;
    assert_eq!(
        list(&room, &no_native_access, &inspection).await.status,
        StatusCode::FORBIDDEN
    );
    inspection.participating = Some(true);
    let empty = list(&room, &manage, &inspection).await;
    assert_eq!(empty.status, StatusCode::OK);
    assert!(ids(&body(&empty)).is_empty());
    assert_eq!(
        body(&empty),
        body(&list(&room, &room.ada.token, &inspection).await)
    );
}

#[tokio::test]
async fn node_chat_lists_keep_stored_previews_and_bound_live_place_names_to_the_same_operation() {
    let room = Room::new().await;
    room.say(&room.cai, room.chat, "The stored node-chat preview")
        .await;
    let primary = rule("OrgChatList.member", "read", "chat", room.chat, "self");
    for extra in [None, Some("ChatGet"), Some("OrgChatList.member")] {
        let mut rules = vec![primary.clone()];
        if let Some(operation) = extra {
            rules.push(rule(operation, "read", "topic", room.topic, "self"));
        }
        let token = key(&room, &room.bea, rules).await;
        let reply = list(&room, &token, &query(ChatListScope::Nodes)).await;
        assert_eq!(reply.status, StatusCode::OK);
        let value = body(&reply);
        assert_eq!(ids(&value), [room.chat]);
        assert!(value.to_string().contains("The stored node-chat preview"));
        assert_eq!(
            value.to_string().contains("Login"),
            extra == Some("OrgChatList.member")
        );
        assert!(
            value["items"][0].get("agents").is_none(),
            "compact native lists stay compact"
        );
        assert!(value["items"][0].get("inspection_holders").is_none());
    }
}

#[tokio::test]
async fn a_checked_empty_member_page_needs_its_own_declared_operation_and_keeps_query_validation() {
    let room = Room::new().await;
    let token = key(
        &room,
        &room.eve,
        vec![rule(
            "OrgChatList.member",
            "read",
            "org",
            room.root,
            "subtree",
        )],
    )
    .await;
    let mut window = query(ChatListScope::Mine);
    let empty = list(&room, &token, &window).await;
    assert_eq!(empty.status, StatusCode::OK);
    assert!(ids(&body(&empty)).is_empty());
    let wrong_operation = key(
        &room,
        &room.eve,
        vec![rule("ChatGet", "read", "org", room.root, "subtree")],
    )
    .await;
    assert_eq!(
        list(&room, &wrong_operation, &window).await.status,
        StatusCode::FORBIDDEN
    );
    window.of = Some(room.root);
    assert_eq!(
        list(&room, &token, &window).await.status,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn an_opened_chat_page_keeps_its_sources_when_new_activity_arrives_and_closes_on_expiry() {
    for expire in [false, true] {
        let room = Room::new().await;
        let older = room.direct(&room.bea, &room.cai).await.base.id;
        room.say(&room.bea, older, "Older conversation").await;
        room.world.advance(Duration::seconds(1));
        let opened = room.direct(&room.bea, &room.gus).await.base.id;
        room.say(&room.bea, opened, "Original selected conversation")
            .await;
        let token = key(
            &room,
            &room.bea,
            vec![rule(
                "OrgChatList.member",
                "read",
                "org",
                room.root,
                "subtree",
            )],
        )
        .await;
        let mut blocker = room.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = room.world.app.clone();
        let root = room.root;
        let request = tokio::spawn(async move {
            let call = app
                .call(OrgChatList, &[&root])
                .token(&token)
                .query(&query(ChatListScope::Mine));
            call.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait, async {
            loop {
                assert!(!request.is_finished(), "the native page must reach activity publication");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')").bind(pid).fetch_one(&room.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual activity wait follows native page preparation");
        room.world.advance(Duration::seconds(1));
        room.say(
            &room.bea,
            older,
            "Newer activity must not replace the selected source",
        )
        .await;
        if expire {
            room.world.advance(Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert!(
                !String::from_utf8_lossy(&reply.raw).contains("Original selected conversation")
            );
        } else {
            assert_eq!(reply.status, StatusCode::OK);
            assert_eq!(ids(&body(&reply)), [opened]);
            assert!(
                body(&reply)
                    .to_string()
                    .contains("Original selected conversation")
            );
        }
    }
}

#[tokio::test]
async fn a_hidden_original_chat_cannot_borrow_another_readable_list_result_at_publication() {
    let room = Room::new().await;
    let another = room.direct(&room.bea, &room.cai).await.base.id;
    room.say(&room.bea, another, "Another still-readable conversation")
        .await;
    room.world.advance(Duration::seconds(1));
    let opened = room.direct(&room.bea, &room.gus).await.base.id;
    room.say(&room.bea, opened, "The original private selected preview")
        .await;
    let token = key(
        &room,
        &room.bea,
        vec![rule(
            "OrgChatList.member",
            "read",
            "org",
            room.root,
            "subtree",
        )],
    )
    .await;
    let mut blocker = room.world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = room.world.app.clone();
    let root = room.root;
    let request = tokio::spawn(async move {
        let call = app
            .call(OrgChatList, &[&root])
            .token(&token)
            .query(&query(ChatListScope::Mine));
        call.send_bytes().await
    });
    let wait = std::time::Duration::from_secs(30);
    tokio::time::timeout(wait, async {
        loop {
            assert!(!request.is_finished(), "the original native page must reach activity publication");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')").bind(pid).fetch_one(&room.world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the actual native page opened before its publication wait");
    room.world
        .app
        .call(ChatStateSet, &[&opened])
        .token(&room.bea.token)
        .body(&ChatStateSetBody {
            level: None,
            hidden: Some(true),
        })
        .send()
        .await
        .ok();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
    assert!(
        reply.status.is_client_error(),
        "the original row left its native list"
    );
    assert!(!String::from_utf8_lossy(&reply.raw).contains("The original private selected preview"));
    let current = list(&room, &room.bea.token, &query(ChatListScope::Mine)).await;
    assert_eq!(ids(&body(&current)), [another]);
}

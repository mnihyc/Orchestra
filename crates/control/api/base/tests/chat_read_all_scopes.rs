//! Native aggregate read-position selection and its real rollbackable state-row waits.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "native aggregate state assertions"
)]
mod chat_world;
mod places;
mod support;

use chat_world::Room;
use chrono::Duration;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::chats::{ChatLevel, ChatStateSet, ChatStateSetBody, OrgChatsRead};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity;
use orch_types::{Id, Json, new_id};
use serde_json::json;

const WAIT: std::time::Duration = std::time::Duration::from_secs(30);

fn rule(operation: &str, action: &str, kind: &str, id: Id, reach: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"resources":["chat"],
        "allow":[{"operation":operation,"actions":[action]}]})
}
fn writes(ids: &[Id]) -> Vec<Json> {
    ids.iter()
        .map(|id| rule("OrgChatsRead", "write", "chat", *id, "self"))
        .collect()
}
async fn key(room: &Room, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *room.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: room.bea.id(),
            name: "Native all-read fixture",
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
async fn mark(room: &Room, token: &str) -> orch_api_core::testing::RawReply {
    room.world
        .app
        .call(OrgChatsRead, &[&room.root])
        .token(token)
        .send_bytes()
        .await
}
async fn state(room: &Room, chat: Id) -> Option<(i64, i64, String)> {
    sqlx::query_as("SELECT last_read_seq,read_revision,xmin::text FROM chat_states WHERE user_id=$1 AND chat_id=$2")
        .bind(room.bea.id()).bind(chat).fetch_optional(&room.world.db.pool).await.unwrap()
}
async fn make_state(room: &Room, chat: Id, level: Option<ChatLevel>) {
    room.world
        .app
        .call(ChatStateSet, &[&chat])
        .token(&room.bea.token)
        .body(&ChatStateSetBody {
            level: level.map(Some),
            hidden: Some(false),
        })
        .send()
        .await
        .ok();
}
async fn absent(room: &Room, chat: Id) {
    // Missing personal rows are a normal native virtual-state shape before first interaction.
    sqlx::query("DELETE FROM chat_states WHERE user_id=$1 AND chat_id=$2")
        .bind(room.bea.id())
        .bind(chat)
        .execute(&room.world.db.pool)
        .await
        .unwrap();
    assert!(state(room, chat).await.is_none());
}
async fn wait_for_state(
    room: &Room,
    pid: i32,
    insert: bool,
    request: &tokio::task::JoinHandle<orch_api_core::testing::RawReply>,
) {
    tokio::time::timeout(WAIT, async {
        loop {
            assert!(!request.is_finished(), "the native state prefix must reach its real SQL wait");
            let pattern = if insert { "%INSERT INTO chat_states (user_id,chat_id) VALUES%" } else { "%FROM chat_states WHERE user_id=%FOR UPDATE%" };
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND (query LIKE $2 OR (NOT $3 AND query LIKE '%INSERT INTO chat_states (user_id,chat_id) VALUES%')))")
                .bind(pid).bind(pattern).bind(insert).fetch_one(&room.world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the native personal-state wait is visible at its actual SQL");
}

#[tokio::test]
async fn all_read_needs_its_own_write_on_every_selected_chat_and_returns_the_native_empty_ack() {
    let room = Room::new().await;
    let one = room.direct(&room.bea, &room.cai).await.base.id;
    let two = room.direct(&room.bea, &room.gus).await.base.id;
    room.say(&room.cai, one, "First native unread").await;
    room.say(&room.gus, two, "Second native unread").await;
    absent(&room, one).await;
    absent(&room, two).await;
    for rules in [
        writes(&[one]),
        vec![rule("OrgChatsRead", "read", "org", room.root, "subtree")],
        vec![rule("ChatRead", "write", "org", room.root, "subtree")],
    ] {
        let secret = key(&room, rules).await;
        assert_eq!(mark(&room, &secret).await.status, StatusCode::FORBIDDEN);
        assert!(
            state(&room, one).await.is_none() && state(&room, two).await.is_none(),
            "no scope-filtered partial write"
        );
    }
    let secret = key(&room, writes(&[one, two])).await;
    let done = mark(&room, &secret).await;
    assert_eq!(done.status, StatusCode::NO_CONTENT);
    assert!(done.raw.is_empty());
    for chat in [one, two] {
        let (read, revision, _) = state(&room, chat).await.unwrap();
        assert!(read > 0);
        assert_eq!(revision, 1);
    }
    assert!(
        room.inbox(&room.bea, "chat.message")
            .await
            .iter()
            .all(|row| row.3)
    );
}

#[tokio::test]
async fn all_read_uses_native_following_and_readability_instead_of_every_chat_under_the_org() {
    let room = Room::new().await;
    let wanted = room.direct(&room.bea, &room.cai).await.base.id;
    let muted = room.direct(&room.bea, &room.gus).await.base.id;
    room.say(&room.cai, wanted, "Followed private unread").await;
    room.say(&room.gus, muted, "Muted private unread").await;
    room.say(&room.cai, room.chat, "Unfollowed node unread")
        .await;
    make_state(&room, muted, Some(ChatLevel::Off)).await;
    let muted_before = state(&room, muted).await;
    let node_before = state(&room, room.chat).await;
    let secret = key(&room, writes(&[wanted])).await;
    assert_eq!(mark(&room, &secret).await.status, StatusCode::NO_CONTENT);
    assert!(state(&room, wanted).await.unwrap().0 > 0);
    assert_eq!(state(&room, muted).await, muted_before);
    assert_eq!(state(&room, room.chat).await, node_before);
}

#[tokio::test]
async fn empty_all_read_keeps_native_view_and_its_own_checked_empty_write_without_creating_state() {
    let room = Room::new().await;
    let secret = key(
        &room,
        vec![rule("OrgChatsRead", "write", "org", room.root, "subtree")],
    )
    .await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM chat_states WHERE user_id=$1")
        .bind(room.bea.id())
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap();
    let done = mark(&room, &secret).await;
    assert_eq!(done.status, StatusCode::NO_CONTENT);
    assert!(done.raw.is_empty());
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM chat_states WHERE user_id=$1")
        .bind(room.bea.id())
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, after);
    let wrong = key(
        &room,
        vec![rule("ChatRead", "write", "org", room.root, "subtree")],
    )
    .await;
    assert_eq!(mark(&room, &wrong).await.status, StatusCode::FORBIDDEN);
    let outsider = new_token(API_TOKEN_PREFIX);
    let scope = json!({"rules":[rule("OrgChatsRead","write","org",room.root,"subtree")]});
    identity::create_api_token_scoped(
        &mut *room.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: room.eve.id(),
            name: "No native org view",
            token_hash: &token_hash(&outsider),
            scopes: &[],
            node_id: None,
            at: room.world.now(),
            expires_at: Some(room.world.now() + Duration::hours(1)),
        },
        Some(&scope),
    )
    .await
    .unwrap();
    assert_eq!(mark(&room, &outsider).await.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn all_read_rechecks_after_the_native_existing_state_row_wait_before_positions_change() {
    for expire in [false, true] {
        let room = Room::new().await;
        let chat = room.direct(&room.bea, &room.cai).await.base.id;
        room.say(&room.cai, chat, "Unread before existing-state wait")
            .await;
        make_state(&room, chat, None).await;
        let before = state(&room, chat).await.unwrap();
        let secret = key(&room, writes(&[chat])).await;
        let mut blocker = room.world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT chat_id FROM chat_states WHERE user_id=$1 AND chat_id=$2 FOR UPDATE")
            .bind(room.bea.id())
            .bind(chat)
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
            let call = app.call(OrgChatsRead, &[&root]).token(&secret);
            call.send_bytes().await
        });
        wait_for_state(&room, pid, false, &request).await;
        assert!(!request.is_finished());
        if expire {
            room.world.advance(Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        assert_eq!(
            reply.status,
            if expire {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::NO_CONTENT
            }
        );
        let after = state(&room, chat).await.unwrap();
        if expire {
            assert_eq!(after, before);
        } else {
            assert!(after.0 > before.0);
            assert_eq!(after.1, before.1 + 1);
        }
    }
}

#[tokio::test]
async fn all_read_missing_state_prefix_wait_is_rollbackable_and_is_rechecked_before_the_first_read_update()
 {
    for expire in [false, true] {
        let room = Room::new().await;
        let chat = room.direct(&room.bea, &room.cai).await.base.id;
        room.say(&room.cai, chat, "Unread before missing-state wait")
            .await;
        absent(&room, chat).await;
        let secret = key(&room, writes(&[chat])).await;
        let mut blocker = room.world.db.pool.begin().await.unwrap();
        sqlx::query("INSERT INTO chat_states (user_id,chat_id) VALUES ($1,$2)")
            .bind(room.bea.id())
            .bind(chat)
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
            let call = app.call(OrgChatsRead, &[&root]).token(&secret);
            call.send_bytes().await
        });
        wait_for_state(&room, pid, true, &request).await;
        assert!(!request.is_finished());
        if expire {
            room.world.advance(Duration::hours(2));
        }
        blocker.rollback().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        assert_eq!(
            reply.status,
            if expire {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::NO_CONTENT
            }
        );
        if expire {
            assert!(
                state(&room, chat).await.is_none(),
                "the staged missing state is rolled back"
            );
        } else {
            let after = state(&room, chat).await.unwrap();
            assert!(after.0 > 0);
            assert_eq!(after.1, 1);
        }
    }
}

#[tokio::test]
async fn all_read_cannot_drop_an_original_target_when_its_key_narrows_during_a_native_state_wait() {
    let room = Room::new().await;
    let one = room.direct(&room.bea, &room.cai).await.base.id;
    let two = room.direct(&room.bea, &room.gus).await.base.id;
    room.say(&room.cai, one, "One original unread").await;
    room.say(&room.gus, two, "Another original unread").await;
    make_state(&room, one, None).await;
    make_state(&room, two, None).await;
    let before_one = state(&room, one).await;
    let before_two = state(&room, two).await;
    let secret = key(&room, writes(&[one, two])).await;
    let hash = token_hash(&secret);
    let last = one.max(two);
    let mut blocker = room.world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT chat_id FROM chat_states WHERE user_id=$1 AND chat_id=$2 FOR UPDATE")
        .bind(room.bea.id())
        .bind(last)
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
        let call = app.call(OrgChatsRead, &[&root]).token(&secret);
        call.send_bytes().await
    });
    wait_for_state(&room, pid, false, &request).await;
    let scope = json!({"rules":writes(&[last])});
    tokio::time::timeout(
        WAIT,
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE token_hash=$1")
            .bind(hash)
            .bind(scope)
            .execute(&room.world.db.pool),
    )
    .await
    .unwrap()
    .unwrap();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(state(&room, one).await, before_one);
    assert_eq!(state(&room, two).await, before_two);
}

#[tokio::test]
async fn later_unread_chats_do_not_replace_the_original_all_read_selection() {
    let room = Room::new().await;
    let old = room.direct(&room.bea, &room.cai).await.base.id;
    let later = room.direct(&room.bea, &room.gus).await.base.id;
    room.say(&room.cai, old, "The original selected unread")
        .await;
    make_state(&room, old, None).await;
    let secret = key(&room, writes(&[old])).await;
    let mut blocker = room.world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT chat_id FROM chat_states WHERE user_id=$1 AND chat_id=$2 FOR UPDATE")
        .bind(room.bea.id())
        .bind(old)
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
        let call = app.call(OrgChatsRead, &[&root]).token(&secret);
        call.send_bytes().await
    });
    wait_for_state(&room, pid, false, &request).await;
    room.say(&room.gus, later, "Unread after the act's selection")
        .await;
    let later_before = state(&room, later).await;
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert!(state(&room, old).await.unwrap().0 > 0);
    assert_eq!(state(&room, later).await, later_before);
}

#[tokio::test]
async fn legacy_all_read_keeps_its_native_root_view_gate_without_a_new_content_read_requirement() {
    let room = Room::new().await;
    let chat = room.direct(&room.bea, &room.cai).await.base.id;
    room.say(&room.cai, chat, "Native legacy bookkeeping").await;
    let legacy = new_token(API_TOKEN_PREFIX);
    identity::create_api_token(
        &mut *room.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: room.bea.id(),
            name: "Legacy all-read root view",
            token_hash: &token_hash(&legacy),
            scopes: &["org.view".into()],
            node_id: None,
            at: room.world.now(),
            expires_at: Some(room.world.now() + Duration::hours(1)),
        },
    )
    .await
    .unwrap();
    let done = mark(&room, &legacy).await;
    assert_eq!(done.status, StatusCode::NO_CONTENT);
    assert!(done.raw.is_empty());
    assert!(state(&room, chat).await.unwrap().0 > 0);
}

#[tokio::test]
async fn completed_all_read_keeps_the_empty_ack_after_activity_wait_expiry_without_repeating_positions()
 {
    let room = Room::new().await;
    let chat = room.direct(&room.bea, &room.cai).await.base.id;
    room.say(&room.cai, chat, "Committed read position").await;
    absent(&room, chat).await;
    let secret = key(&room, writes(&[chat])).await;
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
        let call = app.call(OrgChatsRead, &[&root]).token(&secret);
        call.send_bytes().await
    });
    tokio::time::timeout(WAIT,async {
        loop {
            assert!(!request.is_finished(),"the committed act must reach activity publication");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&room.world.db.pool).await.unwrap();
            if blocked {break;}
            tokio::task::yield_now().await;
        }
    }).await.expect("the real post-handler activity wait is reached");
    let committed = state(&room, chat).await.unwrap();
    assert!(committed.0 > 0);
    assert_eq!(committed.1, 1);
    room.world.advance(Duration::hours(2));
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert!(reply.raw.is_empty());
    assert_eq!(state(&room, chat).await.unwrap(), committed);
}

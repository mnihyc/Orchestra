//! Chat opening and direct replay keep their native outcomes under exact key bounds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "native creation scope controls"
)]
mod chat_world;
mod places;
mod support;

use chat_world::Room;
use chrono::Duration;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::access::{Holder, HolderInput, PermissionPageUpdate};
use orch_api_contract::chats::{ChatStateSet, ChatStateSetBody, OrgChatCreate, OrgChatCreateBody};
use orch_api_contract::directory::{GroupHolderAdd, OrgGroupCreate, OrgGroupCreateBody};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity;
use orch_types::{Id, Json, new_id};
use places::Who;
use serde_json::json;
use support::FullPermissionPageReply as _;

fn write(root: Id) -> Json {
    json!({"target":{"kind":"org","id":root,"reach":"self"},"resources":["chat"],
        "allow":[{"operation":"OrgChatCreate","actions":["write"]}]})
}
fn read(operation: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]})
}
fn full(root: Id) -> Json {
    json!({"target":{"kind":"org","id":root,"reach":"subtree"},
        "allow":[{"operation":"OrgChatCreate","actions":["write","read"]}]})
}
async fn key(room: &Room, actor: &Who, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *room.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: actor.id(),
            name: "Native opening fixture",
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
fn body(room: &Room, group: bool) -> OrgChatCreateBody {
    OrgChatCreateBody {
        user_ids: vec![room.cai.id()],
        name: group.then(|| "Supplied group name".into()),
    }
}
async fn open(room: &Room, token: &str, group: bool) -> orch_api_core::testing::RawReply {
    room.world
        .app
        .call(OrgChatCreate, &[&room.root])
        .token(token)
        .body(&body(room, group))
        .send_bytes()
        .await
}
async fn count(room: &Room) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM chats WHERE org_id=$1 AND created_by=$2")
        .bind(room.root)
        .bind(room.bea.id())
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn opening_write_only_keeps_new_and_replayed_native_status_without_conversation_content() {
    let room = Room::new().await;
    let other = places::make_org(&room.world, &room.ada, "Other root", "other-root")
        .await
        .base
        .id;
    let before = count(&room).await;
    let mut other_operation = write(room.root);
    other_operation["allow"][0]["operation"] = json!("ChatUpdate");
    let mut read_only = write(room.root);
    read_only["allow"][0]["actions"] = json!(["read"]);
    for rule in [write(other), other_operation, read_only] {
        let secret = key(&room, &room.bea, vec![rule]).await;
        assert_eq!(
            open(&room, &secret, true).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(count(&room).await, before);
    }
    let secret = key(&room, &room.bea, vec![write(room.root)]).await;
    let group = open(&room, &secret, true).await;
    assert_eq!(group.status, StatusCode::CREATED);
    let receipt: Json = serde_json::from_slice(&group.raw).unwrap();
    let id: Id = receipt["chat_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        receipt,
        json!({"projection":"receipt","chat_id":id,"created":true})
    );
    assert!(!String::from_utf8_lossy(&group.raw).contains("Supplied group name"));
    assert_eq!(count(&room).await, before + 1);
    let direct = room.direct(&room.bea, &room.cai).await.base.id;
    room.say(&room.cai, direct, "An existing private direct message")
        .await;
    let with_other_read = key(
        &room,
        &room.bea,
        vec![write(room.root), read("ChatGet", "chat", direct)],
    )
    .await;
    let replay = open(&room, &with_other_read, false).await;
    assert_eq!(replay.status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Json>(&replay.raw).unwrap(),
        json!({"projection":"receipt","chat_id":direct,"created":false})
    );
    let complete = key(&room, &room.bea, vec![full(room.root)]).await;
    let replay = open(&room, &complete, false).await;
    assert_eq!(replay.status, StatusCode::OK);
    let shown: Json = serde_json::from_slice(&replay.raw).unwrap();
    assert!(shown.get("projection").is_none());
    assert!(String::from_utf8_lossy(&replay.raw).contains("An existing private direct message"));
    let outsider = key(&room, &room.eve, vec![write(room.root)]).await;
    assert_eq!(
        open(&room, &outsider, true).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn opening_readback_binds_the_actual_result_and_native_inspection_group_sources() {
    let room = Room::new().await;
    let group = room
        .world
        .app
        .call(OrgGroupCreate, &[&room.root])
        .token(&room.ada.token)
        .body(&OrgGroupCreateBody {
            name: "Protected actual inspectors".into(),
            description: Some("Native full group source".into()),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("an ordinary group creation returns its native content");
    room.world
        .app
        .call(GroupHolderAdd, &[&group.id])
        .token(&room.ada.token)
        .body(&orch_api_contract::directory::GroupHolderAddBody {
            holder: HolderInput::User { id: room.cai.id() },
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    room.world.app.call(PermissionPageUpdate, &[&"nodes", &room.root]).token(&room.ada.token)
        .raw_body(&json!({"rows":[{"permission":"directs.view","add":[{"holder":{"kind":"group","id":group.id}}]}]}))
        .send().await.full_permission_page().ok();
    let wanted = room.direct(&room.bea, &room.cai).await;
    assert!(wanted.inspection_holders.as_ref().unwrap().iter().any(|holder|matches!(holder, Holder::Group { group: value } if value.id==group.id && value.member_count>0)));
    let other = room.direct(&room.bea, &room.gus).await;
    for extra in [
        vec![read("OrgChatCreate", "chat", other.base.id)],
        vec![
            read("OrgChatCreate", "chat", wanted.base.id),
            read("ChatGet", "group", group.id),
        ],
    ] {
        let mut rules = vec![write(room.root)];
        rules.extend(extra);
        rules.push(read("OrgChatCreate", "org", room.root));
        let token = key(&room, &room.bea, rules).await;
        let reply = open(&room, &token, false).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<Json>(&reply.raw).unwrap(),
            json!({"projection":"receipt","chat_id":wanted.base.id,"created":false})
        );
    }
    let token = key(
        &room,
        &room.bea,
        vec![
            write(room.root),
            read("OrgChatCreate", "chat", wanted.base.id),
            read("OrgChatCreate", "group", group.id),
            read("OrgChatCreate", "org", room.root),
        ],
    )
    .await;
    let reply = open(&room, &token, false).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&reply.raw).contains("Protected actual inspectors"));
    assert!(
        serde_json::from_slice::<Json>(&reply.raw)
            .unwrap()
            .get("projection")
            .is_none()
    );
}

#[tokio::test]
async fn opening_preparation_rechecks_the_same_key_after_its_actual_native_parent_wait() {
    for expire in [false, true] {
        let room = Room::new().await;
        let secret = key(&room, &room.bea, vec![write(room.root)]).await;
        let before = count(&room).await;
        let mut blocker = room.world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
            .bind(room.root)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = room.world.app.clone();
        let root = room.root;
        let body = body(&room, true);
        let request = tokio::spawn(async move {
            let call = app.call(OrgChatCreate, &[&root]).token(&secret).body(&body);
            call.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait, async {
            loop {
                assert!(!request.is_finished(), "the native parent lock must be reached");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%SELECT id FROM nodes WHERE id=%FOR SHARE%')")
                    .bind(pid).fetch_one(&room.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the real root parent serializes native chat creation");
        if expire {
            room.world.advance(Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        assert_eq!(
            reply.status,
            if expire {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::CREATED
            }
        );
        assert_eq!(count(&room).await, before + i64::from(!expire));
    }
}

#[tokio::test]
async fn completed_direct_reopen_keeps_its_receipt_after_activity_wait_without_repeating_the_effect()
 {
    let room = Room::new().await;
    let direct = room.direct(&room.bea, &room.cai).await.base.id;
    room.say(
        &room.cai,
        direct,
        "Private existing preview for the completed reopen",
    )
    .await;
    room.world
        .app
        .call(ChatStateSet, &[&direct])
        .token(&room.bea.token)
        .body(&ChatStateSetBody {
            level: None,
            hidden: Some(true),
        })
        .send()
        .await
        .ok();
    let hidden: bool =
        sqlx::query_scalar("SELECT hidden FROM chat_states WHERE user_id=$1 AND chat_id=$2")
            .bind(room.bea.id())
            .bind(direct)
            .fetch_one(&room.world.db.pool)
            .await
            .unwrap();
    assert!(hidden);
    let token = key(&room, &room.bea, vec![full(room.root)]).await;
    let before = count(&room).await;
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
    let body = body(&room, false);
    let request = tokio::spawn(async move {
        let call = app.call(OrgChatCreate, &[&root]).token(&token).body(&body);
        call.send_bytes().await
    });
    let wait = std::time::Duration::from_secs(30);
    tokio::time::timeout(wait, async {
        loop {
            assert!(!request.is_finished(), "the completed opening must reach activity publication");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&room.world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the real direct reopen committed before its activity wait");
    let (hidden, xmin): (bool, String) =
        sqlx::query_as("SELECT hidden,xmin::text FROM chat_states WHERE user_id=$1 AND chat_id=$2")
            .bind(room.bea.id())
            .bind(direct)
            .fetch_one(&room.world.db.pool)
            .await
            .unwrap();
    assert!(!hidden, "the existing native private unhide committed");
    room.world.advance(Duration::hours(2));
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Json>(&reply.raw).unwrap(),
        json!({"projection":"receipt","chat_id":direct,"created":false})
    );
    let later: String =
        sqlx::query_scalar("SELECT xmin::text FROM chat_states WHERE user_id=$1 AND chat_id=$2")
            .bind(room.bea.id())
            .bind(direct)
            .fetch_one(&room.world.db.pool)
            .await
            .unwrap();
    assert_eq!(later, xmin);
    assert_eq!(count(&room).await, before);
}

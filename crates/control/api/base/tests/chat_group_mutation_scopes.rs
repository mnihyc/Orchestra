//! Native group writes keep membership rules, truthful acknowledgements and current lock boundaries.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod chat_world;
mod places;
mod support;

use chat_world::Room;
use orch_api_contract::chats::{
    ChatGet, ChatManagerSet, ChatManagerSetBody, ChatMemberAdd, ChatMemberRemove, ChatUpdate,
};
use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
use orch_types::{Id, new_id};
use serde_json::{Value, json};

fn rule(op: &str, chat: Id, actions: &[&str]) -> Value {
    json!({"target":{"kind":"chat","id":chat,"reach":"self"},"allow":[{"operation":op,"actions":actions}]})
}
fn read_tree(op: &str, root: Id) -> Value {
    json!({"target":{"kind":"org","id":root,"reach":"subtree"},"resources":["org","project","topic","thread","chat","group"],"allow":[{"operation":op,"actions":["read"]}]})
}
struct Key {
    id: Id,
    secret: String,
}
async fn key(room: &Room, user: Id, rules: Option<Vec<Value>>, life: chrono::Duration) -> Key {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
    let scope = rules.map(|rules| {
        let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        serde_json::to_value(scope).unwrap()
    });
    let secret = new_token(API_TOKEN_PREFIX);
    let id = new_id();
    let legacy = vec!["*".to_owned()];
    create_api_token_scoped(
        &mut *room.world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Group mutation control",
            token_hash: &token_hash(&secret),
            scopes: if scope.is_some() { &[] } else { &legacy },
            node_id: None,
            at: room.world.now(),
            expires_at: Some(room.world.now() + life),
        },
        scope.as_ref(),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn scoped(room: &Room, user: Id, rules: Vec<Value>) -> Key {
    // Fixed-clock finite HTTP controls use the established short fixture-key lifetime.
    key(room, user, Some(rules), chrono::Duration::hours(1)).await
}
fn result(reply: orch_api_core::testing::RawReply) -> (u16, Value) {
    let body = if reply.raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&reply.raw).unwrap()
    };
    (reply.status.as_u16(), body)
}
fn ok(reply: (u16, Value)) -> Value {
    assert_eq!(reply.0, 200, "{}", reply.1);
    reply.1
}
fn empty(reply: (u16, Value)) {
    assert_eq!(reply, (204, Value::Null));
}
fn receipt(reply: (u16, Value), chat: Id) {
    assert_eq!(ok(reply), json!({"projection":"receipt","chat_id":chat}));
}
async fn rename(room: &Room, token: &str, chat: Id, name: Option<&str>) -> (u16, Value) {
    result(
        room.world
            .app
            .call(ChatUpdate, &[&chat])
            .token(token)
            .raw_body(&json!({"name":name}))
            .send_bytes()
            .await,
    )
}
async fn add(room: &Room, token: &str, chat: Id, user: Id) -> (u16, Value) {
    result(
        room.world
            .app
            .call(ChatMemberAdd, &[&chat, &user])
            .token(token)
            .send_bytes()
            .await,
    )
}
async fn remove(room: &Room, token: &str, chat: Id, user: Id) -> (u16, Value) {
    result(
        room.world
            .app
            .call(ChatMemberRemove, &[&chat, &user])
            .token(token)
            .send_bytes()
            .await,
    )
}
async fn get(room: &Room, token: &str, chat: Id) -> (u16, Value) {
    result(
        room.world
            .app
            .call(ChatGet, &[&chat])
            .token(token)
            .send_bytes()
            .await,
    )
}
async fn group(room: &Room) -> Id {
    room.group(&room.ada, &[&room.bea, &room.cai], Some("Original group"))
        .await
        .base
        .id
}
async fn managed_group(room: &Room) -> Id {
    let chat = group(room).await;
    let bea = room.bea.id();
    // Mutation controls explicitly give their acting member the native manager flag.
    // Ordinary member/leave controls keep group() and its creator-only initial manager.
    let request = room
        .world
        .app
        .call(ChatManagerSet, &[&chat, &bea])
        .token(&room.ada.token)
        .body(&ChatManagerSetBody { manager: true });
    let _changed = request.send().await.ok();
    chat
}
async fn present(room: &Room, chat: Id, user: Id) -> bool {
    orch_db::queries::chats::members(&mut *room.world.conn().await, chat)
        .await
        .unwrap()
        .iter()
        .any(|member| member.user_id == user && member.active_at(room.world.now()))
}
async fn state(room: &Room, chat: Id) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_object('chat',(SELECT to_jsonb(c) FROM chats c WHERE id=$1),'members',COALESCE((SELECT jsonb_agg(to_jsonb(m) ORDER BY user_id) FROM chat_members m WHERE chat_id=$1),'[]'::jsonb),'messages',COALESCE((SELECT jsonb_agg(to_jsonb(m) ORDER BY seq_created,id) FROM chat_messages m WHERE chat_id=$1),'[]'::jsonb),'audit',(SELECT count(*) FROM audit_log WHERE object_id=$1))")
        .bind(chat).fetch_one(&room.world.db.pool).await.unwrap()
}

#[tokio::test]
async fn rename_keeps_member_null_noop_and_write_receipt_semantics_without_borrowing_get_read() {
    let room = Room::new().await;
    let chat = managed_group(&room).await;
    assert_eq!(
        ok(rename(&room, &room.bea.token, chat, Some("  Native name  ")).await)["name"],
        "Native name"
    );
    let before = state(&room, chat).await;
    ok(rename(&room, &room.bea.token, chat, Some("Native name")).await);
    assert_eq!(
        state(&room, chat).await,
        before,
        "equal normalized name remains a native no-op"
    );
    assert!(ok(rename(&room, &room.ada.token, chat, None).await)["name"].is_null());
    let native_denied = rename(&room, &room.dee.token, chat, Some("Not a member")).await;
    assert!(
        matches!(native_denied.0, 403 | 404),
        "the ordinary nonmember is refused before any scoped comparison"
    );
    let writer = scoped(
        &room,
        room.bea.id(),
        vec![rule("ChatUpdate", chat, &["write"])],
    )
    .await;
    receipt(
        rename(&room, &writer.secret, chat, Some("Accepted name")).await,
        chat,
    );
    assert_eq!(
        ok(get(&room, &room.bea.token, chat).await)["name"],
        "Accepted name"
    );
    let unrelated = scoped(
        &room,
        room.bea.id(),
        vec![
            rule("ChatUpdate", chat, &["write"]),
            rule("ChatGet", chat, &["read"]),
        ],
    )
    .await;
    receipt(
        rename(
            &room,
            &unrelated.secret,
            chat,
            Some("Another accepted name"),
        )
        .await,
        chat,
    );
    let full = scoped(
        &room,
        room.bea.id(),
        vec![
            rule("ChatUpdate", chat, &["write"]),
            read_tree("ChatUpdate", room.root),
        ],
    )
    .await;
    let shown = ok(rename(&room, &full.secret, chat, None).await);
    assert_eq!(shown, ok(get(&room, &room.bea.token, chat).await));
    let denied = scoped(
        &room,
        room.dee.id(),
        vec![rule("ChatUpdate", chat, &["write"])],
    )
    .await;
    let before = state(&room, chat).await;
    assert_eq!(
        rename(&room, &denied.secret, chat, Some("Not a member"))
            .await
            .0,
        native_denied.0
    );
    assert_eq!(state(&room, chat).await, before);
}

#[tokio::test]
async fn add_is_manage_on_the_actual_chat_with_public_person_operands_and_native_existing_member_noop()
 {
    let room = Room::new().await;
    let chat = managed_group(&room).await;
    let native = ok(add(&room, &room.bea.token, chat, room.gus.id()).await);
    assert!(
        native["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|user| user["id"] == json!(room.gus.id()))
    );
    let before = state(&room, chat).await;
    ok(add(&room, &room.bea.token, chat, room.gus.id()).await);
    assert_eq!(state(&room, chat).await, before);
    let manager = scoped(
        &room,
        room.bea.id(),
        vec![rule("ChatMemberAdd", chat, &["manage"])],
    )
    .await;
    receipt(add(&room, &manager.secret, chat, room.dee.id()).await, chat);
    assert!(present(&room, chat, room.dee.id()).await);
    let wrong = scoped(
        &room,
        room.bea.id(),
        vec![rule("ChatMemberAdd", chat, &["write"])],
    )
    .await;
    let before = state(&room, chat).await;
    assert_eq!(add(&room, &wrong.secret, chat, room.cai.id()).await.0, 403);
    assert_eq!(state(&room, chat).await, before);
    let full = scoped(
        &room,
        room.bea.id(),
        vec![
            rule("ChatMemberAdd", chat, &["manage"]),
            read_tree("ChatMemberAdd", room.root),
        ],
    )
    .await;
    assert_eq!(
        ok(add(&room, &full.secret, chat, room.dee.id()).await),
        ok(get(&room, &room.bea.token, chat).await)
    );
}

#[tokio::test]
async fn selfleave_is_its_own_write_and_finishes_without_reauthorizing_departed_membership() {
    let room = Room::new().await;
    let chat = group(&room).await;
    empty(remove(&room, &room.cai.token, chat, room.cai.id()).await);
    assert!(!present(&room, chat, room.cai.id()).await);
    let wrong = scoped(
        &room,
        room.bea.id(),
        vec![rule("ChatMemberRemove.remove", chat, &["manage"])],
    )
    .await;
    let before = state(&room, chat).await;
    assert_eq!(
        remove(&room, &wrong.secret, chat, room.bea.id()).await.0,
        403
    );
    assert_eq!(state(&room, chat).await, before);
    let leave = scoped(
        &room,
        room.bea.id(),
        vec![rule("ChatMemberRemove.leave", chat, &["write"])],
    )
    .await;
    empty(remove(&room, &leave.secret, chat, room.bea.id()).await);
    assert!(!present(&room, chat, room.bea.id()).await);
    assert_eq!(
        get(&room, &room.bea.token, chat).await.0,
        404,
        "completed leave does not imply later Read"
    );
}

#[tokio::test]
async fn removing_another_member_is_manage_but_still_requires_the_native_creator_or_moderator() {
    let room = Room::new().await;
    let chat = group(&room).await;
    assert_eq!(
        remove(&room, &room.bea.token, chat, room.cai.id()).await.0,
        403
    );
    empty(remove(&room, &room.ada.token, chat, room.cai.id()).await);
    assert!(!present(&room, chat, room.cai.id()).await);
    let wrong = scoped(
        &room,
        room.ada.id(),
        vec![rule("ChatMemberRemove.leave", chat, &["write"])],
    )
    .await;
    let before = state(&room, chat).await;
    assert_eq!(
        remove(&room, &wrong.secret, chat, room.bea.id()).await.0,
        403
    );
    assert_eq!(state(&room, chat).await, before);
    let kick = scoped(
        &room,
        room.ada.id(),
        vec![rule("ChatMemberRemove.remove", chat, &["manage"])],
    )
    .await;
    empty(remove(&room, &kick.secret, chat, room.bea.id()).await);
    assert!(!present(&room, chat, room.bea.id()).await);
}

#[tokio::test]
async fn disabled_direct_sending_refuses_rename_add_but_keeps_native_leave_and_permitted_removal() {
    let room = Room::new().await;
    let first = managed_group(&room).await;
    let second = group(&room).await;
    room.world
        .app
        .call(NodeSettingSet, &[&room.root, &"chat.direct"])
        .token(&room.ada.token)
        .body(&NodeSettingSetBody {
            value: json!("off"),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        rename(&room, &room.bea.token, first, Some("Disabled"))
            .await
            .0,
        403
    );
    assert_eq!(
        add(&room, &room.bea.token, first, room.gus.id()).await.0,
        403
    );
    empty(remove(&room, &room.bea.token, first, room.bea.id()).await);
    empty(remove(&room, &room.ada.token, first, room.cai.id()).await);
    let leave = scoped(
        &room,
        room.cai.id(),
        vec![rule("ChatMemberRemove.leave", second, &["write"])],
    )
    .await;
    empty(remove(&room, &leave.secret, second, room.cai.id()).await);
    let kick = scoped(
        &room,
        room.ada.id(),
        vec![rule("ChatMemberRemove.remove", second, &["manage"])],
    )
    .await;
    empty(remove(&room, &kick.secret, second, room.bea.id()).await);
}

#[tokio::test]
async fn current_key_narrowing_and_missing_group_membership_never_manufacture_group_authority() {
    let room = Room::new().await;
    let chat = managed_group(&room).await;
    let native = ok(rename(
        &room,
        &room.bea.token,
        chat,
        Some("Before current boundary"),
    )
    .await);
    assert_eq!(native["name"], "Before current boundary");
    let writer = scoped(
        &room,
        room.bea.id(),
        vec![rule("ChatUpdate", chat, &["write"])],
    )
    .await;
    receipt(
        rename(&room, &writer.secret, chat, Some("Current key accepted")).await,
        chat,
    );
    let before = state(&room, chat).await;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(writer.id)
        .bind(json!({"rules":[rule("ChatGet",chat,&["read"])]}))
        .execute(&room.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        rename(&room, &writer.secret, chat, Some("Narrowed"))
            .await
            .0,
        403
    );
    assert_eq!(state(&room, chat).await, before);
    let nonmanager = scoped(
        &room,
        room.cai.id(),
        vec![rule("ChatMemberRemove.remove", chat, &["manage"])],
    )
    .await;
    assert_eq!(
        remove(&room, &nonmanager.secret, chat, room.bea.id())
            .await
            .0,
        403
    );
    assert_eq!(state(&room, chat).await, before);
}

#[derive(Clone, Copy)]
enum Act {
    Rename,
    Add,
    Leave,
    Remove,
}
#[derive(Clone, Copy)]
enum NativeWait {
    ChatRow,
    MembersRead,
}
async fn expires_after_real_chat_wait(which: Act) {
    expires_after_real_native_wait(which, NativeWait::ChatRow).await;
}
async fn expires_after_real_native_wait(which: Act, wait: NativeWait) {
    let room = Room::new().await;
    let chat = if matches!(which, Act::Rename | Act::Add) {
        managed_group(&room).await
    } else {
        group(&room).await
    };
    let actor = if matches!(which, Act::Remove) {
        &room.ada
    } else {
        &room.bea
    };
    // One fake-clock second followed by two is the minimal unambiguous deadline
    // crossing. PostgreSQL blocking is observed, never inferred from a sleep.
    let token = key(&room, actor.id(), None, chrono::Duration::seconds(1)).await;
    match which {
        Act::Rename => {
            ok(rename(&room, &token.secret, chat, Some("Native prime")).await);
        }
        Act::Add => {
            ok(add(&room, &token.secret, chat, room.gus.id()).await);
            empty(remove(&room, &room.ada.token, chat, room.gus.id()).await);
        }
        Act::Leave => {
            empty(remove(&room, &token.secret, chat, room.bea.id()).await);
            ok(add(&room, &room.ada.token, chat, room.bea.id()).await);
        }
        Act::Remove => {
            empty(remove(&room, &token.secret, chat, room.cai.id()).await);
            ok(add(&room, &room.ada.token, chat, room.cai.id()).await);
        }
    }
    let before = state(&room, chat).await;
    let mut held = room.world.db.pool.begin().await.unwrap();
    match wait {
        NativeWait::ChatRow => {
            orch_db::queries::chats::lock(&mut held, chat)
                .await
                .unwrap()
                .unwrap();
        }
        NativeWait::MembersRead => {
            // A relation read wait isolates the native members SELECT after the
            // existing target/identity prefix, without changing any stored facts.
            sqlx::query("LOCK TABLE chat_members IN ACCESS EXCLUSIVE MODE")
                .execute(&mut *held)
                .await
                .unwrap();
        }
    }
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let app = room.world.app.clone();
    let secret = token.secret.clone();
    let bea = room.bea.id();
    let cai = room.cai.id();
    let gus = room.gus.id();
    let action = tokio::spawn(async move {
        match which {
            Act::Rename => {
                let request = app
                    .call(ChatUpdate, &[&chat])
                    .token(&secret)
                    .raw_body(&json!({"name":"Must not commit after expiry"}));
                result(request.send_bytes().await)
            }
            Act::Add => {
                let request = app.call(ChatMemberAdd, &[&chat, &gus]).token(&secret);
                result(request.send_bytes().await)
            }
            Act::Leave => {
                let request = app.call(ChatMemberRemove, &[&chat, &bea]).token(&secret);
                result(request.send_bytes().await)
            }
            Act::Remove => {
                let request = app.call(ChatMemberRemove, &[&chat, &cai]).token(&secret);
                result(request.send_bytes().await)
            }
        }
    });
    let waited=tokio::time::timeout(std::time::Duration::from_secs(10),async{
        loop{
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND (NOT $2 OR query LIKE '%chat_members%'))")
                .bind(blocker).bind(matches!(wait, NativeWait::MembersRead)).fetch_one(&room.world.db.pool).await.unwrap();
            if blocked {
                break true;
            } else if action.is_finished() {
                break false;
            }
            tokio::task::yield_now().await;
        }
    }).await;
    // Ten seconds is below the deployed15s statement bound; it is scheduler
    // headroom for observing this real row wait, not permission freshness time.
    room.world.advance(chrono::Duration::seconds(2));
    held.commit().await.unwrap();
    let reply = action.await.unwrap();
    assert!(
        waited.unwrap(),
        "the request must reach its actual selected native blocker"
    );
    assert_eq!(reply.0, 401, "{}", reply.1);
    assert_eq!(
        state(&room, chat).await,
        before,
        "expired credentials cannot change any group/message/member/sequence/audit state"
    );
}
#[tokio::test]
async fn rename_deadline_is_current_after_the_actual_chat_row_wait() {
    expires_after_real_chat_wait(Act::Rename).await;
}
#[tokio::test]
async fn add_deadline_is_current_after_the_actual_chat_row_wait() {
    expires_after_real_chat_wait(Act::Add).await;
}
#[tokio::test]
async fn leave_deadline_is_current_after_the_actual_chat_row_wait() {
    expires_after_real_chat_wait(Act::Leave).await;
}
#[tokio::test]
async fn removal_deadline_is_current_after_the_actual_chat_row_wait() {
    expires_after_real_chat_wait(Act::Remove).await;
}

#[tokio::test]
async fn native_member_read_wait_cannot_carry_a_group_action_past_its_actual_key_deadline() {
    expires_after_real_native_wait(Act::Rename, NativeWait::MembersRead).await;
}

//! Current group managers, their handover, current waits and exact scoped acknowledgements.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "native group manager controls"
)]
mod chat_world;
mod places;
mod support;

use chat_world::Room;
use orch_api_contract::chats::{
    Chat, ChatAgentPostingSet, ChatAgentPostingSetBody, ChatChangeResponse, ChatGet,
    ChatManagerSet, ChatManagerSetBody, ChatMemberAdd, ChatMemberRemove, ChatUpdate,
    ChatUpdateBody,
};
use orch_api_contract::common::ErrorCode;
use orch_api_core::testing::Reply;
use orch_db::queries::{chats, threads};
use orch_types::{Id, new_id};
use places::Who;
use serde_json::{Value, json};

async fn set(
    room: &Room,
    actor: &Who,
    chat: Id,
    user: Id,
    manager: bool,
) -> Reply<ChatChangeResponse> {
    room.world
        .app
        .call(ChatManagerSet, &[&chat, &user])
        .token(&actor.token)
        .body(&ChatManagerSetBody { manager })
        .send()
        .await
}
async fn get(room: &Room, chat: Id, actor: &Who) -> Chat {
    room.world
        .app
        .call(ChatGet, &[&chat])
        .token(&actor.token)
        .send()
        .await
        .ok()
}
async fn group(room: &Room) -> Id {
    room.group(&room.bea, &[&room.gus], Some("Manager handover"))
        .await
        .base
        .id
}

#[tokio::test]
async fn a_manager_hands_over_without_the_historic_creator_retaining_group_control() {
    let room = Room::new().await;
    let chat = group(&room).await;
    let initial = get(&room, chat, &room.bea).await;
    assert_eq!(initial.managers, [room.bea.id()]);
    assert!(initial.can.manage && initial.can.moderate);
    assert!(!get(&room, chat, &room.gus).await.can.manage);
    assert_eq!(
        set(&room, &room.gus, chat, room.gus.id(), true)
            .await
            .problem()
            .code,
        ErrorCode::ChatManagerRequired
    );
    let shared = set(&room, &room.bea, chat, room.gus.id(), true)
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(shared.managers.len(), 2);
    // The existing duplicate membership outcome must not reset a manager flag.
    room.world
        .app
        .call(ChatMemberAdd, &[&chat, &room.gus.id()])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    assert!(
        get(&room, chat, &room.gus)
            .await
            .managers
            .contains(&room.gus.id())
    );
    set(&room, &room.gus, chat, room.bea.id(), false).await.ok();
    let old = get(&room, chat, &room.bea).await;
    assert_eq!(old.created_by.as_ref().unwrap().id, room.bea.id());
    assert!(!old.can.manage && !old.can.moderate);
    assert_eq!(old.managers, [room.gus.id()]);
    let body = ChatUpdateBody {
        visibility: None,
        name: Some(Some("The next name".into())),
        home_id: None,
    };
    assert_eq!(
        room.world
            .app
            .call(ChatUpdate, &[&chat])
            .token(&room.bea.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::ChatManagerRequired
    );
    assert_eq!(
        room.world
            .app
            .call(ChatMemberAdd, &[&chat, &room.cai.id()])
            .token(&room.bea.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::ChatManagerRequired
    );
    assert_eq!(
        room.world
            .app
            .call(ChatMemberRemove, &[&chat, &room.gus.id()])
            .token(&room.bea.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::ChatManagerRequired
    );
    let changed = room
        .world
        .app
        .call(ChatUpdate, &[&chat])
        .token(&room.gus.token)
        .body(&body)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(changed.base.name.as_deref(), Some("The next name"));
    let invited = room
        .world
        .app
        .call(ChatMemberAdd, &[&chat, &room.fay.id()])
        .token(&room.gus.token)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert!(invited.guests.contains(&room.fay.id()));
    assert!(!invited.managers.contains(&room.fay.id()));
    // Posting stops use the same live manager, without borrowing the original creator.
    let thread = new_id();
    assert!(
        threads::insert_thread(
            &mut *room.world.conn().await,
            thread,
            room.topic,
            "Posting origin",
            room.ada.id(),
            room.world.now()
        )
        .await
        .unwrap()
    );
    assert_eq!(
        room.world
            .app
            .call(ChatAgentPostingSet, &[&chat, &thread])
            .token(&room.bea.token)
            .body(&ChatAgentPostingSetBody { stopped: true })
            .send()
            .await
            .status,
        403
    );
    room.world
        .app
        .call(ChatAgentPostingSet, &[&chat, &thread])
        .token(&room.gus.token)
        .body(&ChatAgentPostingSetBody { stopped: true })
        .send()
        .await
        .ok();
    assert!(
        chats::posting::is_stopped(&mut *room.world.conn().await, chat, thread)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn the_last_manager_cannot_demote_or_remove_themself_and_a_handover_allows_leaving() {
    let room = Room::new().await;
    let chat = group(&room).await;
    let before = get(&room, chat, &room.bea).await;
    assert_eq!(
        set(&room, &room.bea, chat, room.bea.id(), false)
            .await
            .problem()
            .code,
        ErrorCode::ChatLastManager
    );
    assert_eq!(
        room.world
            .app
            .call(ChatMemberRemove, &[&chat, &room.bea.id()])
            .token(&room.bea.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::ChatLastManager
    );
    assert_eq!(get(&room, chat, &room.bea).await.seq, before.seq);
    assert_eq!(
        set(&room, &room.bea, chat, room.eve.id(), true)
            .await
            .status,
        404
    );
    set(&room, &room.bea, chat, room.gus.id(), true).await.ok();
    room.world
        .app
        .call(ChatMemberRemove, &[&chat, &room.bea.id()])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    let after = get(&room, chat, &room.gus).await;
    assert_eq!(after.managers, [room.gus.id()]);
    assert!(
        !after
            .members
            .unwrap()
            .iter()
            .any(|user| user.id == room.bea.id())
    );
}

#[tokio::test]
async fn a_manager_lost_at_the_real_group_lock_cannot_apply_an_earlier_preparation() {
    for losing in [false, true] {
        let room = Room::new().await;
        let chat = group(&room).await;
        set(&room, &room.bea, chat, room.gus.id(), true).await.ok();
        let mut blocker = room.world.db.pool.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        chats::lock(&mut blocker, chat).await.unwrap().unwrap();
        let app = room.world.app.clone();
        let token = room.bea.token.clone();
        let body = ChatUpdateBody {
            visibility: None,
            name: Some(Some("After the wait".into())),
            home_id: None,
        };
        let pending = tokio::spawn(async move {
            let request = app.call(ChatUpdate, &[&chat]).token(&token).body(&body);
            request.send().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async {
            loop {
                assert!(!pending.is_finished(),"the real group row must be the wait");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))")
                    .bind(pid).fetch_one(&room.world.db.pool).await.unwrap();
                if blocked {break;} tokio::task::yield_now().await;
            }
        }).await.unwrap();
        if losing {
            let seq = chats::next_seq(&mut blocker, chat).await.unwrap().unwrap();
            assert!(
                chats::set_manager(
                    &mut blocker,
                    chat,
                    room.bea.id(),
                    false,
                    seq,
                    room.world.now()
                )
                .await
                .unwrap()
            );
            chats::stamp_state(&mut blocker, chat, seq).await.unwrap();
        }
        blocker.commit().await.unwrap();
        let reply = pending.await.unwrap();
        assert_eq!(reply.status, if losing { 403 } else { 200 });
        let actual = chats::chat(&mut *room.world.conn().await, chat)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            actual.name.as_deref(),
            Some(if losing {
                "Manager handover"
            } else {
                "After the wait"
            })
        );
    }
}

async fn key(room: &Room, rules: Vec<Value>) -> String {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity;
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *room.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: room.bea.id(),
            name: "Manager scope fixture",
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
fn rule(operation: &str, chat: Id, actions: &[&str]) -> Value {
    json!({"target":{"kind":"chat","id":chat,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]})
}

#[tokio::test]
async fn manager_changes_need_the_exact_manage_operation_and_keep_write_only_receipts() {
    let room = Room::new().await;
    let chat = group(&room).await;
    for rules in [
        vec![],
        vec![rule("ChatManagerSet", chat, &["write"])],
        vec![rule("ChatMemberAdd", chat, &["manage"])],
        vec![rule("ChatManagerSet", room.chat, &["manage"])],
    ] {
        let denied = key(&room, rules).await;
        assert_eq!(
            room.world
                .app
                .call(ChatManagerSet, &[&chat, &room.gus.id()])
                .token(&denied)
                .body(&ChatManagerSetBody { manager: true })
                .send()
                .await
                .status,
            403
        );
        assert!(
            !chats::is_manager(
                &mut *room.world.conn().await,
                chat,
                room.gus.id(),
                room.world.now()
            )
            .await
            .unwrap()
        );
    }
    let exact = key(&room, vec![rule("ChatManagerSet", chat, &["manage"])]).await;
    let accepted = room
        .world
        .app
        .call(ChatManagerSet, &[&chat, &room.gus.id()])
        .token(&exact)
        .body(&ChatManagerSetBody { manager: true })
        .send()
        .await
        .ok();
    let ChatChangeResponse::Receipt(receipt) = accepted else {
        panic!("Manage alone acknowledges the actual group")
    };
    assert_eq!(receipt.chat_id, chat);
    assert!(
        chats::is_manager(
            &mut *room.world.conn().await,
            chat,
            room.gus.id(),
            room.world.now()
        )
        .await
        .unwrap()
    );
    let readable = key(
        &room,
        vec![rule("ChatManagerSet", chat, &["manage", "read"])],
    )
    .await;
    let full = room
        .world
        .app
        .call(ChatManagerSet, &[&chat, &room.gus.id()])
        .token(&readable)
        .body(&ChatManagerSetBody { manager: false })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(full.managers, [room.bea.id()]);
}

#[tokio::test]
async fn a_pending_join_request_is_decided_by_the_current_manager_after_handover() {
    use orch_api_contract::approvals::{AccessDecision, ApprovalDecide, ApprovalDecision};
    use orch_api_contract::chats::{ChatJoinRequest, ChatJoinRequestBody};
    use support::{FullApprovalReply, FullRequestReply};
    let room = Room::new().await;
    let chat = group(&room).await;
    let request = room
        .world
        .app
        .call(ChatJoinRequest, &[&chat])
        .token(&room.cai.token)
        .body(&ChatJoinRequestBody::default())
        .send()
        .await
        .full_request()
        .ok();
    let id = request.approvals[0].id;
    set(&room, &room.bea, chat, room.gus.id(), true).await.ok();
    set(&room, &room.gus, chat, room.bea.id(), false).await.ok();
    let eligible = orch_engine::membership_requests::deciders(
        &mut *room.world.conn().await,
        chat,
        room.cai.id(),
        orch_types::Time::new(room.world.now()),
    )
    .await
    .unwrap();
    assert!(eligible.contains(&room.gus.id()));
    assert!(!eligible.contains(&room.bea.id()));
    let body = ApprovalDecision::Access(AccessDecision {
        approve: true,
        expires_at: None,
        quick_duration: None,
        permissions: None,
        options: None,
        note: None,
    });
    assert_eq!(
        room.world
            .app
            .call(ApprovalDecide, &[&id])
            .token(&room.bea.token)
            .body(&body)
            .send()
            .await
            .status,
        403
    );
    room.world
        .app
        .call(ApprovalDecide, &[&id])
        .token(&room.gus.token)
        .body(&body)
        .send()
        .await
        .full_approval()
        .ok();
    assert!(
        chats::members(&mut *room.world.conn().await, chat)
            .await
            .unwrap()
            .iter()
            .any(|member| member.user_id == room.cai.id() && member.is_in())
    );
}

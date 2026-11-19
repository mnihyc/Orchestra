//! Actual-database controls for pure destination planning and the checked chat writer.
//! These call the shared producer with an ordinary current person Principal. They do not prove
//! API-key scope admission; the separate ForwardCreate HTTP controls exercise that boundary.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod support;

use chat_world::{Room, text};
use orch_api_contract::chats::{ChatStateSet, ChatStateSetBody};
use orch_authz::{Caller, Principal};
use orch_chat::forward::{self, Delivery, Destination, DestinationState};
use orch_chat::{Acting, Error};
use orch_db::Conn;
use orch_db::audit::Origin;
use orch_db::queries::chats as rows;
use orch_db::secrets::MasterKey;
use orch_types::{Id, Time, new_id};

// These fixtures store no redaction secrets. Fail if that premise changes instead of bypassing masking.
fn no_secret_key() -> Result<&'static MasterKey, String> {
    Err("this fixture has no redaction secret to open".to_owned())
}

async fn principal(room: &Room, conn: &mut Conn) -> Principal {
    Principal::load(
        conn,
        Caller::session(room.ada.id()),
        Time::new(room.world.now()),
    )
    .await
    .unwrap()
}

async fn hide(room: &Room, chat: Id) {
    room.world
        .app
        .call(ChatStateSet, &[&chat])
        .token(&room.ada.token)
        .body(&ChatStateSetBody {
            level: None,
            hidden: Some(true),
        })
        .send()
        .await
        .ok();
}

async fn hidden(conn: &mut Conn, user: Id, chat: Id) -> bool {
    rows::state(conn, user, chat).await.unwrap().unwrap().hidden
}

async fn counts(conn: &mut Conn) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM chats), (SELECT count(*) FROM chat_members),
                (SELECT count(*) FROM chat_states), (SELECT count(*) FROM chat_messages)",
    )
    .fetch_one(conn)
    .await
    .unwrap()
}

async fn message_count(conn: &mut Conn, chat: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM chat_messages WHERE chat_id=$1 AND kind='message'")
        .bind(chat)
        .fetch_one(conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn planning_existing_and_new_directs_changes_no_chat_membership_or_hidden_state() {
    let room = Room::new().await;
    let existing = room.direct(&room.ada, &room.bea).await.base.id;
    hide(&room, existing).await;
    let to = [
        Destination::User(room.bea.id()),
        Destination::User(room.cai.id()),
        Destination::User(room.cai.id()),
        Destination::Chat(existing),
    ];
    let mut conn = room.world.conn().await;
    let before = counts(&mut conn).await;
    assert!(hidden(&mut conn, room.ada.id(), existing).await);
    assert!(
        rows::direct_chat(
            &mut conn,
            room.root,
            &orch_chat::chats::direct_key(room.ada.id(), room.cai.id())
        )
        .await
        .unwrap()
        .is_none()
    );
    // Deliberately no outer rollback: a supposedly pure planner that writes would persist here.
    let plan = forward::destination_plan(&mut conn, room.ada.id(), room.root, &to)
        .await
        .unwrap();
    assert_eq!(plan.len(), to.len());
    assert_eq!(plan[0].state, DestinationState::ExistingChat(existing));
    assert_eq!(
        plan[1].state,
        DestinationState::NewDirect {
            root: room.root,
            user: room.cai.id()
        }
    );
    assert_eq!(
        plan[2], plan[1],
        "duplicate selectors reuse the same pure fact"
    );
    assert_eq!(plan[3].state, DestinationState::ExistingChat(existing));
    assert_eq!(plan.iter().map(|p| p.requested).collect::<Vec<_>>(), to);
    assert_eq!(counts(&mut conn).await, before);
    assert!(hidden(&mut conn, room.ada.id(), existing).await);
    assert!(
        rows::direct_chat(
            &mut conn,
            room.root,
            &orch_chat::chats::direct_key(room.ada.id(), room.cai.id())
        )
        .await
        .unwrap()
        .is_none()
    );
}

#[tokio::test]
async fn an_intervening_direct_creation_refuses_before_posts_and_outer_rollback_restores_provisional_effects()
 {
    let room = Room::new().await;
    let to = [
        Destination::User(room.bea.id()),
        Destination::User(room.cai.id()),
    ];
    let plan = {
        let mut conn = room.world.conn().await;
        let plan = forward::destination_plan(&mut conn, room.ada.id(), room.root, &to)
            .await
            .unwrap();
        assert!(
            plan.iter()
                .all(|p| matches!(p.state, DestinationState::NewDirect { .. }))
        );
        plan
    };
    // A normal API request in another transaction wins after the pure plan. Hiding it gives a
    // visible provisional effect to prove that the later failing open is rolled back too.
    let raced = room.direct(&room.ada, &room.cai).await.base.id;
    hide(&room, raced).await;
    let before = {
        let mut conn = room.world.conn().await;
        assert!(hidden(&mut conn, room.ada.id(), raced).await);
        counts(&mut conn).await
    };
    let mut tx = room.world.app.app.db.begin().await.unwrap();
    let who = principal(&room, &mut tx).await;
    let origin = Origin::default();
    let act = Acting {
        principal: &who,
        now: room.world.now(),
        origin: &origin,
    };
    let client = new_id().to_string();
    let delivery = Delivery {
        root: room.root,
        content: vec![text("must not be posted")],
        only: None,
        to: &to,
        comment: None,
        client_id: &client,
    };
    let refused = forward::deliver_checked(&mut tx, &act, &delivery, &no_secret_key, &plan, &[])
        .await
        .unwrap_err();
    match refused {
        Error::Destination {
            to: selector,
            error,
        } => {
            assert_eq!(selector, serde_json::json!({"user_id":room.cai.id()}));
            assert!(matches!(*error, Error::StaleDestination));
        }
        other => panic!("expected the changed-destination refusal, got {other:?}"),
    }
    let provisional = rows::direct_chat(
        &mut tx,
        room.root,
        &orch_chat::chats::direct_key(room.ada.id(), room.bea.id()),
    )
    .await
    .unwrap()
    .expect("the first destination really created a provisional direct");
    assert_eq!(
        rows::members(&mut tx, provisional.id).await.unwrap().len(),
        2
    );
    assert!(
        !hidden(&mut tx, room.ada.id(), raced).await,
        "opening the raced direct provisionally unhides it"
    );
    assert_eq!(
        counts(&mut tx).await.3,
        before.3,
        "all destinations resolve before any message is posted"
    );
    tx.rollback().await.unwrap();
    let mut conn = room.world.conn().await;
    assert!(
        rows::direct_chat(
            &mut conn,
            room.root,
            &orch_chat::chats::direct_key(room.ada.id(), room.bea.id())
        )
        .await
        .unwrap()
        .is_none()
    );
    assert!(hidden(&mut conn, room.ada.id(), raced).await);
    assert_eq!(
        counts(&mut conn).await,
        before,
        "provisional chat/member/state rows all roll back"
    );
}

#[tokio::test]
async fn duplicate_user_selectors_in_a_checked_new_batch_make_one_direct_and_one_message() {
    let room = Room::new().await;
    let to = [
        Destination::User(room.bea.id()),
        Destination::User(room.bea.id()),
    ];
    let plan = {
        let mut conn = room.world.conn().await;
        forward::destination_plan(&mut conn, room.ada.id(), room.root, &to)
            .await
            .unwrap()
    };
    assert!(
        plan.iter()
            .all(|p| matches!(p.state, DestinationState::NewDirect { .. }))
    );
    let mut tx = room.world.app.app.db.begin().await.unwrap();
    let who = principal(&room, &mut tx).await;
    let origin = Origin::default();
    let act = Acting {
        principal: &who,
        now: room.world.now(),
        origin: &origin,
    };
    let client = new_id().to_string();
    let delivery = Delivery {
        root: room.root,
        content: vec![text("once")],
        only: None,
        to: &to,
        comment: None,
        client_id: &client,
    };
    let sent = forward::deliver_checked(&mut tx, &act, &delivery, &no_secret_key, &plan, &[])
        .await
        .unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].destination.requested, to[0]);
    assert_eq!(
        sent[0].destination.state,
        DestinationState::NewDirect {
            root: room.root,
            user: room.bea.id()
        }
    );
    assert!(sent[0].written.created);
    let chat = sent[0].chat;
    let message = sent[0].written.message.id;
    tx.commit().await.unwrap();
    let mut conn = room.world.conn().await;
    let actual = rows::direct_chat(
        &mut conn,
        room.root,
        &orch_chat::chats::direct_key(room.ada.id(), room.bea.id()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(actual.id, chat);
    assert_eq!(rows::members(&mut conn, chat).await.unwrap().len(), 2);
    assert_eq!(message_count(&mut conn, chat).await, 1);
    assert_eq!(
        rows::message_by_client(&mut conn, chat, room.ada.id(), &client)
            .await
            .unwrap()
            .unwrap()
            .id,
        message
    );
}

#[tokio::test]
async fn existing_direct_and_explicit_chat_aliases_deduplicate_and_valid_opening_still_unhides() {
    let room = Room::new().await;
    let direct = room.direct(&room.ada, &room.bea).await.base.id;
    hide(&room, direct).await;
    let to = [
        Destination::User(room.bea.id()),
        Destination::Chat(direct),
        Destination::User(room.bea.id()),
        Destination::Chat(room.chat),
    ];
    let plan = {
        let mut conn = room.world.conn().await;
        forward::destination_plan(&mut conn, room.ada.id(), room.root, &to)
            .await
            .unwrap()
    };
    let mut tx = room.world.app.app.db.begin().await.unwrap();
    let who = principal(&room, &mut tx).await;
    let origin = Origin::default();
    let act = Acting {
        principal: &who,
        now: room.world.now(),
        origin: &origin,
    };
    let client = new_id().to_string();
    let delivery = Delivery {
        root: room.root,
        content: vec![text("one per actual chat")],
        only: None,
        to: &to,
        comment: None,
        client_id: &client,
    };
    let sent = forward::deliver_checked(&mut tx, &act, &delivery, &no_secret_key, &plan, &[])
        .await
        .unwrap();
    assert_eq!(
        sent.iter().map(|s| s.chat).collect::<Vec<_>>(),
        [direct, room.chat],
        "result order follows first requested targets, not sorted lock/write order"
    );
    assert_eq!(sent[0].destination.requested, to[0]);
    assert_eq!(
        sent[0].destination.state,
        DestinationState::ExistingChat(direct)
    );
    assert_eq!(sent[1].destination.requested, to[3]);
    tx.commit().await.unwrap();
    let mut conn = room.world.conn().await;
    assert!(!hidden(&mut conn, room.ada.id(), direct).await);
    assert_eq!(message_count(&mut conn, direct).await, 1);
    assert_eq!(message_count(&mut conn, room.chat).await, 1);
}

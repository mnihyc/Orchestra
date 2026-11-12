//! Group placement uses the same native chat, actual home and guarded mutation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "native chat placement controls"
)]
mod chat_world;
mod places;
mod support;

use chat_world::Room;
use orch_api_contract::chats::{
    Chat, ChatGet, ChatKind, ChatUpdate, ChatUpdateBody, OrgChatCreate, OrgChatCreateBody,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::everything::{EverythingList, EverythingListQuery};
use orch_db::queries::{chats, hierarchy, threads};
use orch_types::{Id, new_id};

async fn key(room: &Room, rules: serde_json::Value) -> String {
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity;
    let scope: orch_access_contract::key_scope::KeyScope =
        serde_json::from_value(serde_json::json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *room.world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: room.ada.id(),
            name: "Group home scope fixture",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: room.world.now(),
            expires_at: Some(room.world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::json!(scope)),
    )
    .await
    .unwrap();
    secret
}
fn rule(
    operation: &str,
    kind: &str,
    id: Id,
    resources: &[&str],
    actions: &[&str],
) -> serde_json::Value {
    serde_json::json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":resources,
        "allow":[{"operation":operation,"actions":actions}]})
}

async fn make(room: &Room, home: Id) -> Chat {
    room.world
        .app
        .call(OrgChatCreate, &[&room.root])
        .token(&room.ada.token)
        .body(&OrgChatCreateBody {
            visibility: None,
            user_ids: vec![room.bea.id()],
            name: Some("Release room".into()),
            home_id: Some(home),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
}
async fn in_branch(room: &Room, parent: Id, chat: Id) -> bool {
    room.world
        .app
        .call(EverythingList, &[])
        .token(&room.ada.token)
        .query(&EverythingListQuery {
            parent: Some(parent.to_string()),
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
async fn a_group_has_an_independent_home_and_keeps_the_nodes_one_default_chat() {
    let room = Room::new().await;
    let group = make(&room, room.topic).await;
    assert_eq!(group.base.kind, ChatKind::Group);
    assert!(group.base.node.is_none());
    assert_eq!(group.base.home.as_ref().unwrap().id, room.topic);
    assert_eq!(group.base.org_id, room.root);
    let mut conn = room.world.conn().await;
    let stored = chats::chat(&mut conn, group.base.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.home_id, Some(room.topic));
    assert!(stored.node_id.is_none());
    assert_eq!(
        chats::chat_of_node(&mut conn, room.topic)
            .await
            .unwrap()
            .unwrap()
            .id,
        room.chat
    );
    drop(conn);
    assert!(in_branch(&room, room.topic, group.base.id).await);
    assert!(!in_branch(&room, room.project, group.base.id).await);
    assert_eq!(
        room.direct(&room.bea, &room.cai).await.base.id,
        room.direct(&room.cai, &room.bea).await.base.id
    );
}

#[tokio::test]
async fn moving_a_group_keeps_its_name_and_members_and_can_return_it_to_the_root_home() {
    let room = Room::new().await;
    let group = make(&room, room.topic).await;
    let moved = room
        .world
        .app
        .call(ChatUpdate, &[&group.base.id])
        .token(&room.ada.token)
        .body(&ChatUpdateBody {
            visibility: None,
            name: None,
            home_id: Some(Some(room.project)),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(moved.base.name, group.base.name);
    assert_eq!(moved.members, group.members);
    assert_eq!(moved.base.home.unwrap().id, room.project);
    assert!(in_branch(&room, room.project, group.base.id).await);
    assert!(!in_branch(&room, room.topic, group.base.id).await);
    let root = room
        .world
        .app
        .call(ChatUpdate, &[&group.base.id])
        .token(&room.ada.token)
        .body(&ChatUpdateBody {
            visibility: None,
            name: Some(None),
            home_id: Some(None),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert!(root.base.name.is_none());
    assert_eq!(root.base.home.unwrap().id, room.root);
    assert_eq!(
        chats::chat(&mut *room.world.conn().await, group.base.id)
            .await
            .unwrap()
            .unwrap()
            .home_id,
        None
    );
}

#[tokio::test]
async fn a_group_move_needs_chat_view_at_its_destination_and_never_changes_its_root() {
    let room = Room::new().await;
    let group = room
        .group(&room.bea, &[&room.cai], Some("Keep this name"))
        .await;
    let private = places::make_topic(&room.world, &room.ada, room.project, "Private destination")
        .await
        .base
        .id;
    room.inherit_chat_area(private, false).await;
    let denied = room
        .world
        .app
        .call(ChatUpdate, &[&group.base.id])
        .token(&room.bea.token)
        .body(&ChatUpdateBody {
            visibility: None,
            name: None,
            home_id: Some(Some(private)),
        })
        .send()
        .await;
    assert!(!denied.status.is_success());
    let other = places::make_org(&room.world, &room.bea, "Other home", "other-chat-home").await;
    let refused = room
        .world
        .app
        .call(ChatUpdate, &[&group.base.id])
        .token(&room.bea.token)
        .body(&ChatUpdateBody {
            visibility: None,
            name: None,
            home_id: Some(Some(other.base.id)),
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ChatMoveCrossesOrg);
    assert!(
        serde_json::to_string(&refused)
            .unwrap()
            .contains("A chat stays in its organization. Make a new chat there and share a link.")
    );
    let kept = room
        .world
        .app
        .call(ChatGet, &[&group.base.id])
        .token(&room.bea.token)
        .send()
        .await
        .ok();
    assert_eq!(kept.base.name, group.base.name);
    assert_eq!(kept.base.org_id, room.root);
    assert_eq!(kept.base.home.unwrap().id, room.root);
}

#[tokio::test]
async fn a_group_under_a_thread_follows_the_threads_current_place_without_a_copied_path() {
    let room = Room::new().await;
    let thread = new_id();
    assert!(
        threads::insert_thread(
            &mut *room.world.conn().await,
            thread,
            room.topic,
            "Working thread",
            room.ada.id(),
            room.world.now()
        )
        .await
        .unwrap()
    );
    let group = make(&room, thread).await;
    assert!(in_branch(&room, thread, group.base.id).await);
    let next = places::make_project(&room.world, &room.ada, room.root, "Next project")
        .await
        .base
        .id;
    let mut tx = room.world.db.pool.begin().await.unwrap();
    assert!(
        hierarchy::lock_move_rows(&mut tx, room.topic, next)
            .await
            .unwrap()
    );
    hierarchy::move_under(&mut tx, room.topic, next, room.world.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let reply = room
        .world
        .app
        .call(EverythingList, &[])
        .token(&room.ada.token)
        .query(&EverythingListQuery {
            parent: Some(thread.to_string()),
            kinds: Some("chat".into()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let row = reply
        .items
        .iter()
        .find(|row| row.id == group.base.id.to_string())
        .unwrap();
    assert_eq!(row.parent_id.as_deref(), Some(thread.to_string().as_str()));
    assert!(row.path.iter().any(|node| node.id == next));
    assert!(!row.path.iter().any(|node| node.id == room.project));
    assert_eq!(
        chats::chat(&mut *room.world.conn().await, group.base.id)
            .await
            .unwrap()
            .unwrap()
            .home_id,
        Some(thread)
    );
}

#[tokio::test]
async fn scoped_group_creation_binds_the_actual_home_and_a_move_needs_its_own_destination_read() {
    let room = Room::new().await;
    let old_root = key(
        &room,
        serde_json::json!([rule(
            "OrgChatCreate",
            "org",
            room.root,
            &["chat"],
            &["write"]
        )]),
    )
    .await;
    let body = OrgChatCreateBody {
        visibility: None,
        user_ids: vec![room.bea.id()],
        name: Some("Scoped room".into()),
        home_id: Some(room.topic),
    };
    assert_eq!(
        room.world
            .app
            .call(OrgChatCreate, &[&room.root])
            .token(&old_root)
            .body(&body)
            .send()
            .await
            .status,
        403
    );
    let exact = key(
        &room,
        serde_json::json!([rule(
            "OrgChatCreate",
            "topic",
            room.topic,
            &["chat"],
            &["write"]
        )]),
    )
    .await;
    let created = room
        .world
        .app
        .call(OrgChatCreate, &[&room.root])
        .token(&exact)
        .body(&body)
        .send()
        .await
        .ok();
    let orch_api_contract::chats::ChatCreateResponse::Receipt(receipt) = created else {
        panic!("a write-only key gets only the accepted identity")
    };
    let original = rule("ChatUpdate", "chat", receipt.chat_id, &["chat"], &["write"]);
    let move_body = ChatUpdateBody {
        visibility: None,
        name: None,
        home_id: Some(Some(room.project)),
    };
    for extra in [
        Vec::new(),
        vec![rule(
            "NodeGet",
            "project",
            room.project,
            &["project"],
            &["read"],
        )],
    ] {
        let mut rules = vec![original.clone()];
        rules.extend(extra);
        let denied = key(&room, serde_json::json!(rules)).await;
        assert_eq!(
            room.world
                .app
                .call(ChatUpdate, &[&receipt.chat_id])
                .token(&denied)
                .body(&move_body)
                .send()
                .await
                .status,
            403
        );
        assert_eq!(
            chats::chat(&mut *room.world.conn().await, receipt.chat_id)
                .await
                .unwrap()
                .unwrap()
                .home_id,
            Some(room.topic)
        );
    }
    let allowed = key(
        &room,
        serde_json::json!([
            original,
            rule(
                "ChatUpdate",
                "project",
                room.project,
                &["project"],
                &["read"]
            )
        ]),
    )
    .await;
    let moved = room
        .world
        .app
        .call(ChatUpdate, &[&receipt.chat_id])
        .token(&allowed)
        .body(&move_body)
        .send()
        .await
        .ok();
    assert!(matches!(
        moved,
        orch_api_contract::chats::ChatChangeResponse::Receipt(_)
    ));
    let actual = chats::chat(&mut *room.world.conn().await, receipt.chat_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.home_id, Some(room.project));
    assert_eq!(actual.name.as_deref(), Some("Scoped room"));
}

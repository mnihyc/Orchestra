//! Audited direct targets keep exact key bounds and their existing business authority.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod admin_support;
mod chat_world;
mod places;
mod support;

use http::StatusCode;
use orch_api_contract::access::HolderInput;
use orch_api_contract::admin::{
    AdminUserOrgInvitationGive, AdminUserOrgInvitationTakeBack, AdminUserResetLinkCreate,
    AdminUserResetLinkCreateBody,
};
use orch_api_contract::chats::{
    ChatAgentPostingSet, ChatAgentPostingSetBody, ChatDraftDelete, ChatDraftGet, ChatDraftSet,
    ChatDraftSetBody, ChatPart, ChatRead, ChatReadBody, ChatSyncGet, ChatSyncGetQuery,
};
use orch_api_contract::directory::{
    GroupDelete, GroupHolderRemove, GroupMemberAdd, GroupMemberList, GroupMemberRemove,
    OrgGroupCreate, OrgGroupCreateBody,
};
use orch_api_contract::users::UserGet;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::{chats, identity, threads};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;

async fn key(world: &World, user: Id, target: Json, allow: Json) -> String {
    let policy: orch_access_contract::key_scope::KeyScope =
        serde_json::from_value(json!({"rules":[{
            "target":target,"allow":allow,
        }]}))
        .unwrap();
    assert!(policy.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let stored = serde_json::to_value(policy).unwrap();
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Typed path control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: world.now(),
            // The existing one-hour fixture lifetime fits place bounds without moving time.
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&stored),
    )
    .await
    .unwrap();
    secret
}

fn selected(kind: &str, id: Id) -> Json {
    json!({"kind":kind,"id":id,"reach":"self"})
}

async fn group(world: &World, owner: &places::Who, org: Id, name: &str) -> Id {
    world
        .app
        .call(OrgGroupCreate, &[&org])
        .token(&owner.token)
        .body(&OrgGroupCreateBody {
            name: name.into(),
            description: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this ordinary control requires the full native creation response")
        .id
}

#[tokio::test]
async fn all_five_group_adapters_preserve_membership_rules_and_exact_group_bounds() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let member = places::person(&world, "member").await;
    let org = places::make_org(&world, &owner, "Workspace", "workspace")
        .await
        .base
        .id;
    places::join(&world, org, org, &member, "viewer", &owner).await;
    let a = group(&world, &owner, org, "A").await;
    let b = group(&world, &owner, org, "B").await;
    let allow = json!([
        {"operation":"GroupMemberAdd","actions":["manage"]},
        {"operation":"GroupMemberRemove","actions":["manage"]},
        {"operation":"GroupHolderRemove","actions":["manage"]},
        {"operation":"GroupDelete","actions":["manage"]},
        {"operation":"GroupMemberList","actions":["read"]},
    ]);
    let token = key(&world, owner.id(), selected("group", a), allow.clone()).await;
    assert_eq!(
        world
            .app
            .call(GroupMemberAdd, &[&a, &member.id()])
            .token(&token)
            .send()
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    let members = world
        .app
        .call(GroupMemberList, &[&a])
        .token(&token)
        .send()
        .await
        .ok()
        .items;
    assert!(members.iter().any(|person| person.id == member.id()));
    assert_eq!(
        world
            .app
            .call(GroupMemberAdd, &[&b, &member.id()])
            .token(&token)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let powerless = key(&world, member.id(), selected("group", a), allow).await;
    let denied = world
        .app
        .call(GroupMemberAdd, &[&a, &owner.id()])
        .token(&powerless)
        .send()
        .await;
    assert_eq!(
        denied.status,
        StatusCode::FORBIDDEN,
        "a selected operation does not grant MembersManage"
    );
    let problem: orch_api_contract::common::Problem = serde_json::from_slice(&denied.raw).unwrap();
    assert_eq!(
        problem.code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
    assert!(!problem.extra.contains_key("missing"));
    assert!(!problem.extra.contains_key("object"));
    assert!(
        !String::from_utf8_lossy(&denied.raw).contains("Workspace"),
        "a write-only denial does not disclose the keeper's name"
    );
    let browser_denied = world
        .app
        .call(GroupMemberAdd, &[&a, &owner.id()])
        .token(&member.token)
        .send()
        .await;
    let browser_problem = browser_denied.problem();
    assert_eq!(browser_problem.code, problem.code);
    assert!(
        browser_problem.extra.contains_key("missing"),
        "the ordinary browser keeps its existing detailed refusal"
    );
    assert_eq!(
        world
            .app
            .call(GroupHolderRemove, &[&a])
            .token(&token)
            .body(&HolderInput::User { id: member.id() })
            .send()
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert!(
        world
            .app
            .call(GroupMemberList, &[&a])
            .token(&token)
            .send()
            .await
            .ok()
            .items
            .is_empty(),
        "the same direct membership writer removed the selected holder"
    );
    assert_eq!(
        world
            .app
            .call(GroupMemberAdd, &[&a, &member.id()])
            .token(&token)
            .send()
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        world
            .app
            .call(GroupMemberRemove, &[&a, &member.id()])
            .token(&token)
            .send()
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert!(
        world
            .app
            .call(GroupMemberList, &[&a])
            .token(&token)
            .send()
            .await
            .ok()
            .items
            .is_empty()
    );
    assert_eq!(
        world
            .app
            .call(GroupDelete, &[&a])
            .token(&token)
            .send()
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert!(
        orch_db::queries::access::groups_full(&mut *world.conn().await, &[a])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        orch_db::queries::access::groups_full(&mut *world.conn().await, &[b])
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn chat_controls_use_the_chat_identity_without_granting_thread_control_or_history() {
    let room = chat_world::Room::new().await;
    room.world
        .app
        .call(ChatDraftSet, &[&room.chat])
        .token(&room.ada.token)
        .body(&ChatDraftSetBody {
            parts: vec![ChatPart::Text {
                text: "Private draft".into(),
            }],
            reply_to: None,
        })
        .send()
        .await
        .ok();
    let sync = room
        .world
        .app
        .call(ChatSyncGet, &[&room.chat])
        .token(&room.ada.token)
        .query(&ChatSyncGetQuery::default())
        .send()
        .await
        .ok();
    let origin = new_id();
    assert!(
        threads::insert_thread(
            &mut *room.world.conn().await,
            origin,
            room.topic,
            "Posting origin",
            room.ada.id(),
            room.world.now()
        )
        .await
        .unwrap()
    );
    let token = key(
        &room.world,
        room.ada.id(),
        selected("chat", room.chat),
        json!([
            {"operation":"ChatRead","actions":["write"]},
            {"operation":"ChatDraftDelete","actions":["write"]},
            {"operation":"ChatAgentPostingSet","actions":["manage"]},
        ]),
    )
    .await;
    let read = room
        .world
        .app
        .call(ChatRead, &[&room.chat])
        .token(&token)
        .body(&ChatReadBody {
            seq: sync.seq,
            read_revision: sync.chat.my.as_ref().map_or(0, |state| state.read_revision),
            unread_from: None,
        })
        .send()
        .await;
    assert_eq!(read.status, StatusCode::NO_CONTENT);
    assert!(read.raw.is_empty());
    let discarded = room
        .world
        .app
        .call(ChatDraftDelete, &[&room.chat])
        .token(&token)
        .send()
        .await;
    assert_eq!(discarded.status, StatusCode::NO_CONTENT);
    assert!(discarded.raw.is_empty());
    assert!(
        room.world
            .app
            .call(ChatDraftGet, &[&room.chat])
            .token(&room.ada.token)
            .send()
            .await
            .ok()
            .is_none()
    );
    assert_eq!(
        room.world
            .app
            .call(ChatAgentPostingSet, &[&room.chat, &origin])
            .token(&token)
            .body(&ChatAgentPostingSetBody { stopped: true })
            .send()
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert!(
        chats::posting::is_stopped(&mut *room.world.conn().await, room.chat, origin)
            .await
            .unwrap()
    );
    assert_eq!(
        room.world
            .app
            .call(ChatDraftGet, &[&room.chat])
            .token(&token)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let other = chat_world::chat_of(&room.world, &room.ada, room.root).await;
    assert_eq!(
        room.world
            .app
            .call(ChatDraftDelete, &[&other])
            .token(&token)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn account_targets_keep_public_reads_and_ordinary_administration_distinct() {
    let world = World::new().await;
    let cast = admin_support::cast(&world).await;
    let allow = json!([
        {"operation":"UserGet","actions":["read"]},
        {"operation":"AdminUserResetLinkCreate","actions":["manage"]},
        {"operation":"AdminUserOrgInvitationGive","actions":["manage"]},
        {"operation":"AdminUserOrgInvitationTakeBack","actions":["manage"]},
    ]);
    let token = key(
        &world,
        cast.root.id(),
        selected("account", cast.ada.id()),
        allow.clone(),
    )
    .await;
    assert_eq!(
        world
            .app
            .call(UserGet, &[&cast.ada.id()])
            .token(&token)
            .send()
            .await
            .ok()
            .id,
        cast.ada.id()
    );
    assert_eq!(
        world
            .app
            .call(UserGet, &[&cast.boss.id()])
            .token(&token)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        world
            .app
            .call(AdminUserOrgInvitationGive, &[&cast.ada.id()])
            .token(&token)
            .send()
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert!(
        identity::user_by_id(&mut *world.conn().await, cast.ada.id())
            .await
            .unwrap()
            .unwrap()
            .org_invited_at
            .is_some()
    );
    assert_eq!(
        world
            .app
            .call(AdminUserOrgInvitationTakeBack, &[&cast.ada.id()])
            .token(&token)
            .send()
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert!(
        identity::user_by_id(&mut *world.conn().await, cast.ada.id())
            .await
            .unwrap()
            .unwrap()
            .org_invited_at
            .is_none()
    );
    let reset = world
        .app
        .call(AdminUserResetLinkCreate, &[&cast.ada.id()])
        .token(&token)
        .body(&AdminUserResetLinkCreateBody {
            clear_second_step: Some(false),
        })
        .send()
        .await
        .ok();
    assert!(
        reset.url.as_ref().is_some_and(|url| !url.is_empty()),
        "the newly issued link is the operation's receipt"
    );
    let ordinary = key(
        &world,
        cast.boss.id(),
        selected("account", cast.ada.id()),
        allow,
    )
    .await;
    assert_eq!(
        world
            .app
            .call(AdminUserOrgInvitationGive, &[&cast.ada.id()])
            .token(&ordinary)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let actor = orch_api_core::middleware::authentication::authenticate(
        &world.app.app,
        &[(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        )]
        .into_iter()
        .collect(),
        world.now(),
    )
    .await
    .unwrap();
    if let orch_api_core::middleware::authentication::Caller::User(user) = actor {
        assert!(!user.as_admin);
        assert!(
            !user.is_system_admin(),
            "ordinary operation admission is not private-access override"
        );
    } else {
        panic!("a real stored key authenticates its account");
    }
}

#[tokio::test]
async fn public_account_read_preserves_retained_service_identity_behavior() {
    let world = World::new().await;
    let cast = admin_support::cast(&world).await;
    let service = new_id();
    identity::create_service_account(
        &mut *world.conn().await,
        service,
        cast.acme,
        "retained-service",
        "Retained service",
        cast.boss.id(),
        world.now(),
    )
    .await
    .unwrap();
    identity::disable_service_account(
        &mut *world.conn().await,
        service,
        cast.boss.id(),
        world.now(),
    )
    .await
    .unwrap();
    orch_db::queries::hierarchy::delete_subtree(
        &mut *world.conn().await,
        cast.acme,
        cast.boss.id(),
        world.now(),
    )
    .await
    .unwrap();
    let browser = world
        .app
        .call(UserGet, &[&service])
        .token(&cast.root.token)
        .send()
        .await
        .ok();
    let token = key(
        &world,
        cast.root.id(),
        selected("account", service),
        json!([{"operation":"UserGet","actions":["read"]}]),
    )
    .await;
    let scoped = world
        .app
        .call(UserGet, &[&service])
        .token(&token)
        .send()
        .await
        .ok();
    assert_eq!(scoped.id, browser.id);
    assert_eq!(scoped.display_name, browser.display_name);
}

//! A member's move may give no higher role than the mover can give.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

#[path = "member_giving/expiry.rs"]
mod expiry;
mod places;
mod support;

use orch_api_contract::access::{
    HolderInput, PageChange, PageChangeAdd, PageChangeRow, PermissionPageUpdate, Role,
};
use orch_api_contract::auth::{ApiTokenCreateBody, MeTokenCreate, SensitiveProof};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::ownership::{
    MemberMoveBody, NodeDelete, NodeDeleteBody, NodeDeleteMembers, OrgMemberMove,
    OrgMemberMovePreview,
};
use orch_types::Id;
use places::{Who, join, make_department, make_org, person};
use support::World;

async fn give_row(world: &World, boss: &Who, node: Id, member: Id, permission: &str) {
    world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &node])
        .token(&boss.token)
        .body(&PageChange {
            rows: Some(vec![PageChangeRow {
                permission: permission.to_owned(),
                profile: None,
                inherit: None,
                locked: None,
                add: Some(vec![PageChangeAdd {
                    holder: HolderInput::User { id: member },
                    options: None,
                    expires_at: None,
                    note: None,
                }]),
                update: None,
                remove: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn a_member_manager_cannot_move_someone_with_a_role_above_their_own() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let manager = person(&world, "manager").await;
    let member = person(&world, "member").await;
    let root = make_org(&world, &boss, "Acme", "acme").await.base.id;
    let from = make_department(&world, &boss, root, "From", "from")
        .await
        .base
        .id;
    let to = make_department(&world, &boss, root, "To", "to")
        .await
        .base
        .id;
    join(&world, root, root, &manager, "viewer", &boss).await;
    join(&world, root, from, &member, "viewer", &boss).await;
    for org in [from, to] {
        give_row(&world, &boss, org, manager.id(), "members.manage").await;
    }
    let body = MemberMoveBody {
        to,
        role: Some(Role::Developer),
    };
    let preview = world
        .app
        .call(OrgMemberMovePreview, &[&root, &member.id()])
        .token(&manager.token)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(preview.code, ErrorCode::AuthzDenied);
    let preview = world
        .app
        .call(OrgMemberMovePreview, &[&root, &member.id()])
        .token(&boss.token)
        .body(&body)
        .send()
        .await;
    assert!(preview.status.is_success());
    let denied = world
        .app
        .call(OrgMemberMove, &[&root, &member.id()])
        .token(&manager.token)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    let moved = world
        .app
        .call(OrgMemberMove, &[&root, &member.id()])
        .token(&boss.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(moved.domain.id, to);
    assert_eq!(moved.role, Role::Developer);
}

#[tokio::test]
async fn deleting_a_department_cannot_promote_its_members_above_the_deleter() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let manager = person(&world, "manager").await;
    let member = person(&world, "member").await;
    let root = make_org(&world, &boss, "Acme", "acme").await.base.id;
    let from = make_department(&world, &boss, root, "From", "from")
        .await
        .base
        .id;
    join(&world, root, root, &manager, "viewer", &boss).await;
    join(&world, root, from, &member, "viewer", &boss).await;
    give_row(&world, &boss, root, manager.id(), "members.manage").await;
    give_row(&world, &boss, from, manager.id(), "members.manage").await;
    give_row(&world, &boss, from, manager.id(), "org.manage").await;
    let body = NodeDeleteBody {
        members: Some(NodeDeleteMembers {
            role: Some(Role::Admin),
        }),
        ..NodeDeleteBody::default()
    };
    let denied = world
        .app
        .call(NodeDelete, &[&from])
        .token(&manager.token)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    let deleted = world
        .app
        .call(NodeDelete, &[&from])
        .token(&boss.token)
        .body(&body)
        .send()
        .await;
    assert!(deleted.status.is_success());
}

#[tokio::test]
async fn a_scoped_token_cannot_move_a_member_with_permissions_outside_its_scopes() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let member = person(&world, "member").await;
    let root = make_org(&world, &boss, "Acme", "acme").await.base.id;
    let from = make_department(&world, &boss, root, "From", "from")
        .await
        .base
        .id;
    let to = make_department(&world, &boss, root, "To", "to")
        .await
        .base
        .id;
    join(&world, root, from, &member, "viewer", &boss).await;
    let token = world
        .app
        .call(MeTokenCreate, &[])
        .token(&boss.token)
        .body(&ApiTokenCreateBody {
            name: "members only".to_owned(),
            scopes: Some(vec!["org.view".to_owned(), "members.manage".to_owned()]),
            node_id: Some(root),
            expires_at: None,
            proof: SensitiveProof {
                password: Some(support::PASSWORD.to_owned()),
                ..Default::default()
            },
        })
        .send()
        .await
        .ok()
        .token
        .unwrap();
    let body = MemberMoveBody {
        to,
        role: Some(Role::Developer),
    };
    let denied = world
        .app
        .call(OrgMemberMove, &[&root, &member.id()])
        .token(&token)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    let moved = world
        .app
        .call(OrgMemberMove, &[&root, &member.id()])
        .token(&boss.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(moved.role, Role::Developer);
}

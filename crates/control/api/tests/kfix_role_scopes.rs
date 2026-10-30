//! Scoped role grants, extensions and retractions use current defaults and current transaction authority.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use chrono::Duration;
use orch_api_contract::access::{
    NodeDefaultsUpdate, NodeRoleCreate, PermissionPageUpdate, RoleDelete, RoleUpdate,
};
use orch_api_contract::auth::{ApiTokenCreateBody, MeTokenCreate};
use orch_api_contract::common::ErrorCode;
use orch_types::{Id, Time};
use places::{Who, join, make_org, make_project, person};
use serde_json::json;
use support::World;

async fn token(world: &World, owner: &Who, project: Id, scopes: Vec<String>) -> String {
    world
        .app
        .call(MeTokenCreate, &[])
        .token(&owner.token)
        .body(&ApiTokenCreateBody {
            name: "Bounded role giver".to_owned(),
            scopes: Some(scopes),
            node_id: Some(project),
            expires_at: None,
            proof: orch_api_contract::auth::SensitiveProof {
                password: Some(support::PASSWORD.to_owned()),
                ..Default::default()
            },
        })
        .send()
        .await
        .ok()
        .token
        .expect("the first answer carries the secret")
}

#[tokio::test]
async fn a_view_only_token_cannot_give_a_role_whose_default_permissions_it_lacks() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let recipient = person(&world, "recipient").await;
    let org = make_org(&world, &owner, "Office", "office").await;
    join(
        &world,
        org.base.id,
        org.base.id,
        &recipient,
        "viewer",
        &owner,
    )
    .await;
    let project = make_project(&world, &owner, org.base.id, "Inside").await;
    let credential = token(
        &world,
        &owner,
        project.base.id,
        vec!["project.view".to_owned()],
    )
    .await;
    let problem = world
        .app
        .call(NodeRoleCreate, &[&project.base.id])
        .token(&credential)
        .raw_body(&json!({
            "subject": {"kind": "user", "id": recipient.id()}, "role": "viewer"
        }))
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::AuthzDenied);
    assert!(
        problem.extra["missing"]
            .as_array()
            .unwrap()
            .iter()
            .any(|missing| {
                missing["permissions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|permission| permission == "thread.view")
            })
    );
}

#[tokio::test]
async fn sufficient_scopes_allow_roles_but_changed_defaults_stop_holder_only_updates_and_retractions()
 {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let recipient = person(&world, "recipient").await;
    let org = make_org(&world, &owner, "Office", "office").await;
    join(
        &world,
        org.base.id,
        org.base.id,
        &recipient,
        "viewer",
        &owner,
    )
    .await;
    let project = make_project(&world, &owner, org.base.id, "Inside")
        .await
        .base
        .id;
    world.app.call(PermissionPageUpdate, &[&"nodes", &project]).token(&owner.token).raw_body(&json!({
        "whole": "private", "rows": [{"permission": "project.view", "add": [{"holder": {"kind": "role", "role": "viewer"}}]}]
    })).send().await.ok();
    world.app.call(NodeDefaultsUpdate, &[&project]).token(&owner.token).raw_body(&json!({
        "whole": "private", "rows": [
            {"permission": "thread.view", "profile": "managed", "add": [{"holder": {"kind": "role", "role": "viewer"}}]},
            {"permission": "thread.manage", "profile": "managed", "add": [{"holder": {"kind": "role", "role": "developer"}}]}
        ]
    })).send().await.ok();
    let credential = token(
        &world,
        &owner,
        project,
        vec!["project.view".to_owned(), "thread.view".to_owned()],
    )
    .await;
    let given = world.app.call(NodeRoleCreate, &[&project]).token(&credential).raw_body(&json!({
        "subject": {"kind": "user", "id": recipient.id()}, "role": "viewer", "expires_at": Time::new(world.now() + Duration::hours(1))
    })).send().await.ok();
    world
        .app
        .call(RoleUpdate, &[&given.id])
        .token(&credential)
        .raw_body(&json!({
            "expected_revision": given.revision, "note": "still within the same held role"
        }))
        .send()
        .await
        .ok();
    world.app.call(NodeDefaultsUpdate, &[&project]).token(&owner.token).raw_body(&json!({
        "rows": [{"permission": "thread.interact", "profile": "managed", "add": [{"holder": {"kind": "role", "role": "viewer"}}]}]
    })).send().await.ok();
    let problem = world
        .app
        .call(RoleDelete, &[&given.id])
        .token(&credential)
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::AuthzDenied);
    let problem = world.app.call(RoleUpdate, &[&given.id]).token(&credential).raw_body(&json!({
        "expected_revision": given.revision + 1, "expires_at": Time::new(world.now() + Duration::hours(2))
    })).send().await.problem();
    assert_eq!(problem.code, ErrorCode::AuthzDenied);
    // The page manager can retract without giving every default permission.
    let manager = token(&world, &owner, project, vec!["access.manage".to_owned()]).await;
    world
        .app
        .call(RoleDelete, &[&given.id])
        .token(&manager)
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn a_management_only_token_cannot_lengthen_a_role_into_permission_names_it_does_not_hold() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let recipient = person(&world, "recipient").await;
    let org = make_org(&world, &owner, "Office", "office").await;
    join(
        &world,
        org.base.id,
        org.base.id,
        &recipient,
        "viewer",
        &owner,
    )
    .await;
    let project = make_project(&world, &owner, org.base.id, "Inside")
        .await
        .base
        .id;
    let given = world.app.call(NodeRoleCreate, &[&project]).token(&owner.token).raw_body(&json!({
        "subject": {"kind": "user", "id": recipient.id()}, "role": "viewer", "expires_at": Time::new(world.now() + Duration::hours(2))
    })).send().await.ok();
    let credential = token(&world, &owner, project, vec!["access.manage".to_owned()]).await;
    let shortened = world.app.call(RoleUpdate, &[&given.id]).token(&credential).raw_body(&json!({
        "expected_revision": given.revision, "expires_at": Time::new(world.now() + Duration::hours(1))
    })).send().await.ok();
    let problem = world
        .app
        .call(RoleUpdate, &[&given.id])
        .token(&credential)
        .raw_body(&json!({
            "expected_revision": shortened.revision, "expires_at": null
        }))
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::AuthzDenied);
    assert!(
        problem.extra["missing"]
            .as_array()
            .unwrap()
            .iter()
            .any(|missing| {
                missing["permissions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|permission| permission == "thread.view")
            })
    );
}

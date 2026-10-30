//! Effective-access explanations name the role's source and each administration cause with scoped redaction.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::Desk;
use orch_api_contract::access::{
    AccessThrough, EffectiveAccess, EffectiveAccessGet, EffectiveAccessGetQuery, Holder,
    NodeDefaultsUpdate, PermissionPageUpdate, Role,
};
use orch_api_contract::auth::{ApiTokenCreateBody, MeTokenCreate};
use orch_api_contract::nodes::NodeOrSystem;
use orch_db::queries::resources::access_fixtures;
use orch_types::Id;
use places::Who;
use serde_json::json;

async fn effective(desk: &Desk, who: &Who, object: &str, id: Id) -> EffectiveAccess {
    desk.world
        .app
        .call(EffectiveAccessGet, &[&object, &id])
        .token(&who.token)
        .query(&EffectiveAccessGetQuery { user_id: who.id() })
        .send()
        .await
        .ok()
}

fn management(access: &EffectiveAccess) -> &[AccessThrough] {
    &access
        .permissions
        .iter()
        .find(|held| held.permission == "access.manage")
        .unwrap()
        .through
}

#[tokio::test]
async fn the_same_permission_keeps_role_named_owner_admin_and_system_causes() {
    let desk = Desk::new().await;
    let via_role = desk.member("viarole", "viewer").await;
    let named = desk.member("named", "viewer").await;
    let admin = desk.member("admin", "admin").await;
    let system_admin = places::admin(&desk.world, "systemadmin").await;
    desk.world
        .app
        .call(NodeDefaultsUpdate, &[&desk.project])
        .token(&desk.ada.token)
        .raw_body(&json!({"rows": [{"permission": "access.manage", "add": [{
            "holder": {"kind": "role", "role": "viewer"}
        }]}]}))
        .send()
        .await
        .ok();
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &desk.topic])
        .token(&desk.ada.token)
        .raw_body(&json!({"rows": [{"permission": "access.manage", "add": [{
            "holder": {"kind": "user", "id": named.id()}
        }]}]}))
        .send()
        .await
        .ok();
    let role_access = effective(&desk, &via_role, "nodes", desk.topic).await;
    assert_eq!(role_access.role, Some(Role::Viewer));
    assert!(
        matches!(role_access.role_from, Some(NodeOrSystem::Node(ref place)) if place.id == desk.org && !place.redacted)
    );
    assert!(management(&role_access).iter().any(|cause| matches!(cause,
        AccessThrough::Entry { entry, from: Some(NodeOrSystem::Node(place)), .. }
            if matches!(entry.holder, Holder::Role { role: Role::Viewer, .. }) && place.id == desk.project
    )));
    let named_access = effective(&desk, &named, "nodes", desk.topic).await;
    assert!(management(&named_access).iter().any(|cause| matches!(cause,
        AccessThrough::Entry { entry, from: None, .. }
            if matches!(entry.holder, Holder::User { ref user } if user.id == named.id())
    )));
    let owner_access = effective(&desk, &desk.ada, "nodes", desk.topic).await;
    assert!(management(&owner_access).contains(&AccessThrough::Owner));
    // An admin acts as one with the session's As admin switch on.
    places::admin_mode(&desk.world, &admin, true).await;
    let admin_access = effective(&desk, &admin, "nodes", desk.topic).await;
    assert!(
        matches!(admin_access.role_from, Some(NodeOrSystem::Node(ref place)) if place.id == desk.org)
    );
    assert!(management(&admin_access).iter().any(|cause| matches!(cause,
        AccessThrough::Admin { from: Some(NodeOrSystem::Node(place)) } if place.id == desk.org
    )));
    let system_access = effective(&desk, &system_admin, "nodes", desk.topic).await;
    assert_eq!(system_access.role_from, Some(NodeOrSystem::SYSTEM));
    assert!(management(&system_access).contains(&AccessThrough::Admin {
        from: Some(NodeOrSystem::SYSTEM)
    }));
    let system_profile = {
        let mut conn = desk.world.conn().await;
        access_fixtures::insert_resource(&mut conn, "agent", None, None, "System profile")
            .await
            .unwrap()
    };
    let system_owned = effective(&desk, &system_admin, "agents", system_profile).await;
    assert!(management(&system_owned).contains(&AccessThrough::SystemAdmin));
}

#[tokio::test]
async fn role_sources_keep_their_identity_but_hide_a_private_or_out_of_scope_place() {
    let desk = Desk::new().await;
    let reader = desk.member("reader", "viewer").await;
    places::give_role(&desk.world, desk.project, &reader, "developer", &desk.ada).await;
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &desk.project])
        .token(&desk.ada.token)
        .raw_body(&json!({"whole": "private"}))
        .send()
        .await
        .ok();
    let access = effective(&desk, &reader, "nodes", desk.topic).await;
    assert_eq!(access.role, Some(Role::Developer));
    assert!(
        matches!(access.role_from, Some(NodeOrSystem::Node(ref place))
        if place.id == desk.project && place.redacted && place.name.is_none())
    );
    let scoped = desk
        .world
        .app
        .call(MeTokenCreate, &[])
        .token(&reader.token)
        .body(&ApiTokenCreateBody {
            name: "Scoped explanation".to_owned(),
            scopes: Some(vec!["topic.view".to_owned()]),
            node_id: Some(desk.topic),
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
        .expect("the first answer carries the secret");
    let access = desk
        .world
        .app
        .call(EffectiveAccessGet, &[&"nodes", &desk.topic])
        .token(&scoped)
        .query(&EffectiveAccessGetQuery {
            user_id: reader.id(),
        })
        .send()
        .await
        .ok();
    assert!(
        matches!(access.role_from, Some(NodeOrSystem::Node(ref place))
        if place.id == desk.project && place.redacted && place.name.is_none())
    );
    let permissions: Vec<&str> = access
        .permissions
        .iter()
        .map(|held| held.permission.as_str())
        .collect();
    assert_eq!(permissions, ["topic.view"]);
}

#[tokio::test]
async fn every_entry_of_one_holder_is_explained_by_the_same_chain() {
    // The defaults name a role once for each permission it gives; its chain is found once
    // and every one of those entries still carries it.
    let desk = Desk::new().await;
    let via_role = desk.member("viarole", "viewer").await;
    let access = effective(&desk, &via_role, "nodes", desk.topic).await;
    let mut chains: std::collections::HashMap<String, (usize, serde_json::Value)> =
        std::collections::HashMap::new();
    for cause in access
        .permissions
        .iter()
        .flat_map(|held| held.through.iter())
    {
        if let AccessThrough::Entry { entry, chain, .. } = cause {
            let holder = serde_json::to_value(&entry.holder).unwrap().to_string();
            let chain = serde_json::to_value(chain).unwrap();
            let seen = chains.entry(holder).or_insert((0, chain.clone()));
            assert_eq!(seen.1, chain, "one holder, one chain");
            seen.0 += 1;
        }
    }
    assert!(
        chains.values().any(|(entries, _)| *entries > 1),
        "a role gives several permissions here"
    );
}

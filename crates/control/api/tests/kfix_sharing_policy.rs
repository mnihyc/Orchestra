//! Home and system readers report the resolved sharing rule and agree with actual giving.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod resources;
mod support;

use desk::Desk;
use orch_api_contract::access::{AccessChoice, PermissionPageGet, PermissionPageUpdate};
use orch_api_contract::admin::AdminDefaultsGet;
use orch_api_contract::common::ErrorCode;
use orch_db::queries::resources::access_fixtures;
use orch_settings::Target;
use serde_json::json;
use support::World;

#[tokio::test]
async fn a_nonowner_holders_home_offers_and_actual_giving_follow_shipped_true_and_explicit_false() {
    let desk = Desk::new().await;
    let holder = desk.member("holder", "viewer").await;
    let recipient = desk.member("recipient", "viewer").await;
    let document = desk.make(&desk.ada, "Shared words").await.base.id;
    let provider = {
        let mut conn = desk.world.conn().await;
        access_fixtures::insert_resource(
            &mut conn,
            "provider",
            Some(desk.ada.id()),
            Some(desk.org),
            "Shared key",
        )
        .await
        .unwrap()
    };
    resources::name_person(
        &desk.world,
        "provider",
        provider,
        "access.own",
        desk.ada.id(),
    )
    .await;
    let subjects = [
        ("nodes", desk.project, "project.interact"),
        ("documents", document, "doc.edit"),
        ("providers", provider, "provider.use"),
    ];
    for (object, id, _) in subjects {
        desk.world
            .app
            .call(PermissionPageUpdate, &[&object, &id])
            .token(&desk.ada.token)
            .raw_body(&json!({"whole": "private", "name": [{
                "holders": [{"kind": "user", "id": holder.id()}], "choice": "full_interact"
            }]}))
            .send()
            .await
            .ok();
    }
    for sharing in [true, false] {
        if !sharing {
            desk.world
                .set_at(
                    Target::Node(desk.org),
                    "sharing.allow_holders",
                    json!(false),
                )
                .await;
        }
        for (object, id, permission) in subjects {
            let page = desk
                .world
                .app
                .call(PermissionPageGet, &[&object, &id])
                .token(&holder.token)
                .send()
                .await
                .ok();
            assert_eq!(page.gives.holders_share, sharing, "{object}");
            assert_eq!(
                page.gives.choices.contains(&AccessChoice::FullInteract),
                sharing,
                "{object}"
            );
            assert_eq!(
                page.rows
                    .iter()
                    .find(|row| row.permission == permission)
                    .unwrap()
                    .can_give,
                sharing
            );
            let result = desk
                .world
                .app
                .call(PermissionPageUpdate, &[&object, &id])
                .token(&holder.token)
                .raw_body(&json!({"rows": [{"permission": permission,
                    "add": [{"holder": {"kind": "user", "id": recipient.id()}}]}]}))
                .send()
                .await;
            if sharing {
                result.ok();
            } else {
                assert_eq!(result.problem().code, ErrorCode::AuthzDenied);
            }
        }
    }
}

#[tokio::test]
async fn a_system_resources_nonowner_holder_uses_the_system_policy_instead_of_an_owners_only_fallback()
 {
    let world = World::new().await;
    let holder = places::person(&world, "holder").await;
    let recipient = places::person(&world, "recipient").await;
    let id = {
        let mut conn = world.conn().await;
        access_fixtures::insert_resource(&mut conn, "agent", None, None, "System profile")
            .await
            .unwrap()
    };
    resources::name_person(&world, "agent", id, "agent.use", holder.id()).await;
    for sharing in [true, false] {
        if !sharing {
            world.set("sharing.allow_holders", json!(false)).await;
        }
        let page = world
            .app
            .call(PermissionPageGet, &[&"agents", &id])
            .token(&holder.token)
            .send()
            .await
            .ok();
        assert!(page.owner.is_none());
        assert_eq!(page.gives.holders_share, sharing);
        assert_eq!(
            page.gives.choices.contains(&AccessChoice::FullInteract),
            sharing
        );
        assert_eq!(
            page.rows
                .iter()
                .find(|row| row.permission == "agent.use")
                .unwrap()
                .can_give,
            sharing
        );
        let result = world
            .app
            .call(PermissionPageUpdate, &[&"agents", &id])
            .token(&holder.token)
            .raw_body(&json!({"rows": [{"permission": "agent.use",
                "add": [{"holder": {"kind": "user", "id": recipient.id()}}]}]}))
            .send()
            .await;
        if sharing {
            result.ok();
        } else {
            assert_eq!(result.problem().code, ErrorCode::AuthzDenied);
        }
    }
}

#[tokio::test]
async fn the_system_defaults_reader_reports_the_same_resolved_holders_sharing_policy() {
    let world = World::new().await;
    let admin = places::admin(&world, "admin").await;
    for sharing in [true, false] {
        if !sharing {
            world.set("sharing.allow_holders", json!(false)).await;
        }
        let page = world
            .app
            .call(AdminDefaultsGet, &[])
            .token(&admin.token)
            .send()
            .await
            .ok();
        assert_eq!(page.gives.holders_share, sharing);
    }
}

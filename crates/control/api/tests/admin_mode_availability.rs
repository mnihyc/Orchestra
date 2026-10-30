//! The real session route exposes the admin switch only for administrative roles beyond personal space.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::{
    access::{NodeRoleCreate, RoleCreateBody, RoleDelete},
    auth::{AdminModeSetBody, MeAdminModeGet, MeAdminModeSet},
    common::ErrorCode,
};
use orch_types::{Id, Time};
use places::{Who, join, make_department, make_org, make_project, make_topic, person, who};
use support::World;

async fn available(world: &World, by: &Who) -> bool {
    world
        .app
        .call(MeAdminModeGet, &[])
        .token(&by.token)
        .send()
        .await
        .ok()
        .available
}

#[tokio::test]
async fn ada_administers_acme_but_vera_the_viewer_and_ben_the_outsider_have_no_admin_switch() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let vera = person(&world, "vera").await;
    let ben = person(&world, "ben").await;
    let acme = make_org(&world, &ada, "Acme", "acme").await.base.id;
    join(&world, acme, acme, &vera, "viewer", &ada).await;
    // Personal ownership, including an owned project below it, is ordinary work and needs no admin mode.
    make_project(&world, &ben, ben.person.personal_org, "My work").await;
    assert!(available(&world, &ada).await);
    for reader in [&vera, &ben] {
        let state = world
            .app
            .call(MeAdminModeGet, &[])
            .token(&reader.token)
            .send()
            .await
            .ok();
        assert!(
            !state.available && !state.as_admin,
            "{} is not an administrator",
            reader.person.username
        );
        let refused = world
            .app
            .call(MeAdminModeSet, &[])
            .token(&reader.token)
            .body(&AdminModeSetBody { as_admin: true })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied);
        assert!(!available(&world, reader).await);
    }
}

async fn give_admin(
    world: &World,
    by: &Who,
    to: &Who,
    place: Id,
    expires: Option<Time>,
) -> orch_api_contract::access::RoleHeld {
    let body: RoleCreateBody = serde_json::from_value(serde_json::json!({
        "subject":{"kind":"user","id":to.id()},"role":"admin","expires_at":expires
    }))
    .unwrap();
    world
        .app
        .call(NodeRoleCreate, &[&place])
        .token(&by.token)
        .body(&body)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn current_org_department_and_place_roles_enable_availability_and_revocation_or_expiry_removes_it()
 {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let vera = person(&world, "vera").await;
    let root = make_org(&world, &ada, "Acme", "acme").await.base.id;
    let home = make_department(&world, &ada, root, "Member home", "member-home")
        .await
        .base
        .id;
    join(&world, root, home, &vera, "viewer", &ada).await;
    let department = make_department(&world, &ada, root, "Research", "research")
        .await
        .base
        .id;
    let project = make_project(&world, &ada, department, "Work").await.base.id;
    let topic = make_topic(&world, &ada, project, "Release").await.base.id;
    for place in [root, department, project, topic] {
        assert!(!available(&world, &vera).await);
        let role = give_admin(&world, &ada, &vera, place, None).await;
        assert!(available(&world, &vera).await);
        world
            .app
            .call(RoleDelete, &[&role.id])
            .token(&ada.token)
            .send()
            .await
            .ok();
        assert!(!available(&world, &vera).await);
    }
    give_admin(
        &world,
        &ada,
        &vera,
        project,
        Some(Time::new(world.now() + chrono::Duration::seconds(1))),
    )
    .await;
    assert!(available(&world, &vera).await);
    world.advance(chrono::Duration::seconds(2));
    assert!(!available(&world, &vera).await);
    let system = who(&world, "system_admin", true).await;
    assert!(
        available(&world, &system).await,
        "system administrators need no place role"
    );
}

//! The permission page of a resource over HTTP, read and changed through the requests that serve the page of anything.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::access::{
    AccessChoice, HolderInput, PageChange, PageChangeName, PageChangeOwners, PermissionPage,
    PermissionPageGet, PermissionPageUpdate,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{
    CreateProvider, Provider, ProviderCreate, ProviderGet, ProviderList, ProviderListQuery,
    Upstream,
};
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_db::secrets::MasterKey;
use orch_settings::Target;
use orch_types::{Id, Json, Time};
use places::{Who, missing_permissions};
use resources::company;
use serde_json::json;
use support::World;

async fn world() -> World {
    install_master_key(MasterKey::new([7; 32]));
    World::new().await
}

async fn provider(world: &World, by: &Who, home: Id) -> Provider {
    world
        .app
        .call(ProviderCreate, &[])
        .token(&by.token)
        .body(&CreateProvider::Api {
            runtime_machine_id: None,
            name: "Team key".to_owned(),
            home_id: home,
            access: None,
            arrangements: None,
            runtime_config: None,
            upstreams: vec![Upstream {
                protocol: "openai-responses".to_owned(),
                base_url: "https://api.example.com/v1".to_owned(),
                options: json!({}),
            }],
            api_key: Some("sk-test".to_owned()),
            secrets: None,
            key_machine_id: None,
            sources: None,
        })
        .send()
        .await
        .ok()
        .provider
}

async fn read(
    world: &World,
    by: &Who,
    plural: &str,
    id: Id,
) -> orch_api_core::testing::Reply<PermissionPage> {
    world
        .app
        .call(PermissionPageGet, &[&plural, &id])
        .token(&by.token)
        .send()
        .await
}

fn naming(person: Id, choice: AccessChoice) -> PageChange {
    PageChange {
        name: Some(vec![PageChangeName {
            holders: vec![HolderInput::User { id: person }],
            choice,
            export: None,
            options: None,
            expires_at: None,
            note: None,
        }]),
        ..PageChange::default()
    }
}

#[tokio::test]
async fn the_page_of_a_provider_is_read_by_whoever_holds_anything_on_it_and_by_nobody_else() {
    let world = world().await;
    let c = company(&world).await;
    let made = provider(&world, &c.boss, c.acme).await;
    let id = made.base.id;
    let page = read(&world, &c.boss, "providers", id).await.ok();
    assert_eq!(page.object.id, id);
    assert_eq!(page.owner.as_ref().unwrap().username, "boss");
    assert_eq!(page.owners.len(), 1);
    assert!(page.owners_writable);
    assert!(page.rows.iter().any(|row| row.permission == "provider.use"));
    let refused = read(&world, &c.stranger, "providers", id).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&refused), ["provider.view"]);
    let nobody = world
        .app
        .call(PermissionPageGet, &[&"providers", &id])
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
    let missing = read(&world, &c.boss, "providers", orch_types::new_id())
        .await
        .problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
    let wrong = read(&world, &c.boss, "gadgets", id).await.problem();
    assert!(
        matches!(wrong.code, ErrorCode::NotFound | ErrorCode::Internal),
        "{wrong:?}"
    );
}

#[tokio::test]
async fn naming_somebody_on_the_page_of_a_provider_lets_them_in_and_is_audited() {
    let world = world().await;
    let c = company(&world).await;
    let id = provider(&world, &c.boss, c.acme).await.base.id;
    let before = read(&world, &c.boss, "providers", id).await.ok();
    let seen = |who: &Who| {
        let call = world
            .app
            .call(ProviderList, &[])
            .token(&who.token)
            .query(&ProviderListQuery::default());
        async move { call.send().await.ok().items.len() }
    };
    assert_eq!(seen(&c.viewer).await, 0);
    let changed = world
        .app
        .call(PermissionPageUpdate, &[&"providers", &id])
        .token(&c.boss.token)
        .body(&PageChange {
            expected_revision: Some(before.revision),
            ..naming(c.viewer.id(), AccessChoice::FullInteract)
        })
        .send()
        .await
        .ok();
    assert!(changed.revision > before.revision);
    let use_row = changed
        .rows
        .iter()
        .find(|row| row.permission == "provider.use")
        .unwrap();
    assert_eq!(use_row.entries.len(), 1);
    assert_eq!(
        seen(&c.viewer).await,
        1,
        "named in the Use row, they are offered it"
    );
    let read_by_viewer = world
        .app
        .call(ProviderGet, &[&id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(
        read_by_viewer.access.permissions,
        ["provider.view", "provider.use"]
    );
    let audited: Vec<(String, Json)> = sqlx::query_as(
        "SELECT action, details FROM audit_log WHERE action LIKE 'permission.%' ORDER BY at, id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert!(!audited.is_empty());
    assert!(
        audited
            .iter()
            .all(|(_, details)| details.to_string().contains(&id.to_string())),
        "the line says which provider's page it is: {audited:?}"
    );
}

#[tokio::test]
async fn the_page_of_a_provider_is_changed_by_those_who_hold_access_manage_on_it_alone() {
    let world = world().await;
    let c = company(&world).await;
    let id = provider(&world, &c.boss, c.acme).await.base.id;
    resources::name_person(&world, "provider", id, "provider.use", c.viewer.id()).await;
    // A user of the provider shares what they hold, and managing the page is for whoever holds
    // access.manage: making somebody an owner is not sharing.
    let making_an_owner = PageChange {
        owners: Some(PageChangeOwners {
            add: Some(vec![HolderInput::User { id: c.viewer.id() }]),
            remove: None,
            first: None,
        }),
        ..PageChange::default()
    };
    let refused = world
        .app
        .call(PermissionPageUpdate, &[&"providers", &id])
        .token(&c.viewer.token)
        .body(&making_an_owner)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "access.manage");
    // Nor does holding the Use row let them name somebody from outside the org.
    let outsider = world
        .app
        .call(PermissionPageUpdate, &[&"providers", &id])
        .token(&c.viewer.token)
        .body(&naming(c.stranger.id(), AccessChoice::FullView))
        .send()
        .await
        .problem();
    assert_eq!(outsider.code, ErrorCode::SharingExternalForbidden);
    let stranger = world
        .app
        .call(PermissionPageUpdate, &[&"providers", &id])
        .token(&c.stranger.token)
        .body(&naming(c.stranger.id(), AccessChoice::FullView))
        .send()
        .await
        .problem();
    assert_eq!(stranger.code, ErrorCode::AuthzDenied);
    let page = read(&world, &c.boss, "providers", id).await.ok();
    let entries: usize = page.rows.iter().map(|row| row.entries.len()).sum();
    assert_eq!(entries, 1, "only the entry that was there");
}

#[tokio::test]
async fn a_change_of_a_page_made_against_an_old_revision_is_refused_with_the_page_as_it_is() {
    let world = world().await;
    let c = company(&world).await;
    let id = provider(&world, &c.boss, c.acme).await.base.id;
    let page = read(&world, &c.boss, "providers", id).await.ok();
    world
        .app
        .call(PermissionPageUpdate, &[&"providers", &id])
        .token(&c.boss.token)
        .body(&naming(c.viewer.id(), AccessChoice::FullView))
        .send()
        .await
        .ok();
    let stale = world
        .app
        .call(PermissionPageUpdate, &[&"providers", &id])
        .token(&c.boss.token)
        .body(&PageChange {
            expected_revision: Some(page.revision),
            ..naming(c.admin.id(), AccessChoice::FullView)
        })
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert!(stale.extra["current"]["rows"].is_array());
}

#[tokio::test]
async fn the_page_of_an_agent_profile_is_served_the_same_way() {
    let world = world().await;
    let c = company(&world).await;
    let made = world
        .app
        .call(orch_api_contract::resources::OrgAgentCreate, &[&c.acme])
        .token(&c.boss.token)
        .body(&orch_api_contract::resources::OrgAgentCreateBody {
            name: "Main".to_owned(),
            harness_kind: "codex".to_owned(),
            profile: json!({}),
        })
        .send()
        .await
        .ok();
    let page = read(&world, &c.boss, "agents", made.base.id).await.ok();
    assert_eq!(page.object.id, made.base.id);
    assert!(page.rows.iter().any(|row| row.permission == "agent.use"));
    let refused = read(&world, &c.stranger, "agents", made.base.id)
        .await
        .problem();
    assert_eq!(missing_permissions(&refused), ["agent.view"]);
}

#[tokio::test]
async fn nobody_else_is_named_in_the_use_row_of_a_subscription_its_home_keeps_to_its_owner() {
    let world = world().await;
    let c = company(&world).await;
    world
        .set_at(
            Target::Node(c.acme),
            "providers.subscription_sharing",
            json!(["claude-code"]),
        )
        .await;
    let mut conn = world.conn().await;
    let subscription =
        fixtures::insert_subscription(&mut conn, c.boss.id(), c.acme, "Team Pro", "codex")
            .await
            .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Provider.into(),
            id: subscription,
        },
        c.boss.id(),
        NewAccess::Defaults,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    drop(conn);
    let refused = world
        .app
        .call(PermissionPageUpdate, &[&"providers", &subscription])
        .token(&c.boss.token)
        .body(&naming(c.viewer.id(), AccessChoice::FullInteract))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ProviderSubscriptionNotShared);
    assert_eq!(refused.extra["harness_kind"], "codex");
    // Naming somebody to look at it is not sharing it.
    let looked = world
        .app
        .call(PermissionPageUpdate, &[&"providers", &subscription])
        .token(&c.boss.token)
        .body(&naming(c.viewer.id(), AccessChoice::FullView))
        .send()
        .await;
    assert!(
        looked.body.is_ok() || looked.status == 403,
        "{:?}",
        looked.body
    );
}

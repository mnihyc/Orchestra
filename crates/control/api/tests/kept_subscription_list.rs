//! A subscription kept to its first owner is not listed as usable for a further owner.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::access::{HolderInput, PageChange, PageChangeOwners, PermissionPageUpdate};
use orch_api_contract::resources::{ProviderList, ProviderListQuery, ProviderListScope};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_settings::Target;
use orch_types::Time;
use serde_json::json;
use support::World;

#[tokio::test]
async fn only_the_first_owner_sees_a_kept_subscription_in_the_usable_list() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    world
        .set_at(
            Target::Node(company.acme),
            "providers.subscription_sharing",
            json!([]),
        )
        .await;
    let id = {
        let mut conn = world.conn().await;
        let id = fixtures::insert_subscription(
            &mut conn,
            company.boss.id(),
            company.acme,
            "Personal plan",
            "codex",
        )
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ObjectKind::Provider,
                id,
            },
            company.boss.id(),
            NewAccess::Defaults,
            Time::new(world.now()),
        )
        .await
        .unwrap();
        id
    };
    world
        .app
        .call(PermissionPageUpdate, &[&"providers", &id])
        .token(&company.boss.token)
        .body(&PageChange {
            owners: Some(PageChangeOwners {
                add: Some(vec![HolderInput::User {
                    id: company.viewer.id(),
                }]),
                remove: None,
                first: None,
            }),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    let query = ProviderListQuery {
        scope: Some(ProviderListScope::Usable),
        ..ProviderListQuery::default()
    };
    let first = world
        .app
        .call(ProviderList, &[])
        .token(&company.boss.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert!(
        first
            .items
            .into_iter()
            .filter_map(orch_api_contract::common::Listed::into_visible)
            .any(|provider| provider.base.id == id)
    );
    let further = world
        .app
        .call(ProviderList, &[])
        .token(&company.viewer.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert!(
        !further
            .items
            .into_iter()
            .filter_map(orch_api_contract::common::Listed::into_visible)
            .any(|provider| provider.base.id == id),
        "only its first owner may use this subscription"
    );
}

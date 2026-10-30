//! Provider facts and vendor status text require the current credential's Provider View.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_api_contract::auth::{ApiTokenCreateBody, MeTokenCreate};
use orch_api_contract::resources::{
    CreateProvider, ProviderCreate, ProviderGet, ProviderList, ProviderListQuery, Upstream,
};
use serde_json::json;
use support::World;

async fn credential(world: &World, owner: &places::Who, scope: &str) -> String {
    world
        .app
        .call(MeTokenCreate, &[])
        .token(&owner.token)
        .body(&ApiTokenCreateBody {
            name: "Provider projection".to_owned(),
            scopes: Some(vec![scope.to_owned()]),
            node_id: None,
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
async fn page_administration_and_resource_management_do_not_expose_vendor_facts_without_view() {
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([7; 32]),
    );
    let world = World::new().await;
    let company = resources::company(&world).await;
    let id = world
        .app
        .call(ProviderCreate, &[])
        .token(&company.boss.token)
        .body(&CreateProvider::Api {
            runtime_machine_id: None,
            name: "Private provider".to_owned(),
            home_id: company.acme,
            access: None,
            arrangements: None,
            runtime_config: None,
            upstreams: vec![Upstream {
                protocol: "openai-responses".to_owned(),
                base_url: "https://api.example.com/v1".to_owned(),
                options: json!({}),
            }],
            api_key: Some("test-key".to_owned()),
            secrets: None,
            key_machine_id: None,
            sources: None,
        })
        .send()
        .await
        .ok()
        .provider
        .base
        .id;
    {
        let mut conn = world.conn().await;
        sqlx::query("UPDATE providers SET facts = $2, facts_overrides = $3, facts_at = $4, status_detail = $5 WHERE id = $1")
            .bind(id).bind(json!({"plan": "Private plan", "identity": "private-vendor@example.test"}))
            .bind(json!({"plan": "Private override"})).bind(world.now())
            .bind("Vendor private-vendor@example.test has an account-specific error")
            .execute(&mut *conn).await.unwrap();
    }
    let page_manager = credential(&world, &company.boss, "access.manage").await;
    let resource_manager = credential(&world, &company.boss, "provider.manage").await;
    for token in [&company.admin.token, &page_manager, &resource_manager] {
        let providers = world
            .app
            .call(ProviderList, &[])
            .token(token)
            .query(&ProviderListQuery::default())
            .send()
            .await
            .ok();
        let hidden = providers
            .items
            .iter()
            .find_map(|provider| match provider {
                orch_api_contract::common::Listed::Hidden(row) if row.id == id => Some(row),
                _ => None,
            })
            .unwrap();
        assert_eq!(hidden.name, "Private provider");
        let wire = serde_json::to_value(hidden).unwrap();
        for member in [
            "facts",
            "facts_overridden",
            "facts_at",
            "status_detail",
            "base",
            "access",
            "models",
            "runtime",
        ] {
            assert!(
                wire.get(member).is_none(),
                "{member} is omitted altogether without View"
            );
        }
        assert_eq!(
            world
                .app
                .call(ProviderGet, &[&id])
                .token(token)
                .send()
                .await
                .problem()
                .code,
            orch_api_contract::common::ErrorCode::AuthzDenied
        );
    }
    let viewer = credential(&world, &company.boss, "provider.view").await;
    for token in [&company.boss.token, &viewer] {
        let provider = world
            .app
            .call(ProviderGet, &[&id])
            .token(token)
            .send()
            .await
            .ok();
        assert_eq!(provider.facts.plan.as_deref(), Some("Private override"));
        assert_eq!(
            provider.facts.identity.as_deref(),
            Some("private-vendor@example.test")
        );
        assert_eq!(provider.facts_overridden, ["plan"]);
        assert!(provider.facts_at.is_some());
        assert!(provider.status_detail.is_some());
    }
}

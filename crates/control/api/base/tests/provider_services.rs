//! Explicit service choices use the existing provider management writer and revision.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]
#[path = "provider_services/capability_backends.rs"]
mod capability_backends;
mod places;
mod resources;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{
    CreateProvider, ProviderCreate, ProviderGet, ProviderUpdate, Upstream,
};
use orch_provider_contract::builtins::BuiltinSources;
use orch_provider_contract::capabilities::{
    Binding, Declarations, Endpoint, EndpointRule, Interface, Offering, Price, Support,
};
use orch_types::Json;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use support::World;

fn sources() -> BuiltinSources {
    let mut sources = BuiltinSources::default();
    sources.register_capabilities("fixture-service", Declarations {
        interfaces: BTreeMap::from([("fixture-search".into(), Interface { kind: "search".into(), implementation: "http-json".into(), shape:Binding {request:json!({"method":"POST","path":"/search","body":{"query":{"from_input":"/query"}}}),..Binding::default()}, ..Interface::default() })]),
        endpoint_rules: vec![EndpointRule {
            when: json!({"required":["host"],"properties":{"host":{"const":"api.example.invalid"}}}),
            endpoints: BTreeMap::from([("fixture-service".into(), Endpoint { address: "https://api.example.invalid".into(), interface: "fixture-search".into(), ..Endpoint::default() })]),
            offerings: BTreeMap::from([("fixture-search".into(), Offering {
                endpoint: "fixture-service".into(), kind: "service".into(), groups: vec!["web".into()],
                capabilities: BTreeMap::from([("web_search".into(), Support { enabled: true, ..Support::default() })]),
                prices: BTreeMap::from([("search".into(), Price { amount: "0.005".into(), currency: "USD".into(), unit: "search".into(), ..Price::default() })]),
                ..Offering::default()
            })]),
            ..EndpointRule::default()
        }],
        ..Declarations::default()
    }).unwrap();
    sources
}

#[tokio::test]
async fn an_installed_image_service_is_available_to_enable_without_enabling_an_unknown_transport() {
    let mut sources = sources();
    sources.register_capabilities("fixture-images", Declarations {
        interfaces: ["image-api", "uninstalled-codec"].into_iter().map(|implementation| (
            implementation.into(), Interface {
                kind: "images".into(), implementation: implementation.into(),
                shape: Binding { request: json!({"method":"POST","path":"/images/generations","body":{"prompt":{"from_input":"/prompt"}}}), ..Binding::default() },
                ..Interface::default()
            }
        )).collect(),
        endpoint_rules: vec![EndpointRule {
            when: json!({"required":["host"],"properties":{"host":{"const":"api.example.invalid"}}}),
            endpoints: ["image-api", "uninstalled-codec"].into_iter().map(|implementation| (
                implementation.into(), Endpoint { interface: implementation.into(), address: "https://api.example.invalid".into(), ..Endpoint::default() }
            )).collect(),
            offerings: ["image-api", "uninstalled-codec"].into_iter().map(|implementation| (
                implementation.into(), Offering {
                    endpoint: implementation.into(), kind: "service".into(),
                    capabilities: BTreeMap::from([("image_generation".into(), Support { enabled: true, ..Support::default() })]),
                    ..Offering::default()
                }
            )).collect(),
            ..EndpointRule::default()
        }],
        ..Declarations::default()
    }).unwrap();
    let world = World::with_builtin_sources(Arc::new(sources)).await;
    let firm = resources::company(&world).await;
    let made = world
        .app
        .call(ProviderCreate, &[])
        .token(&firm.boss.token)
        .body(&CreateProvider::Api {
            runtime_machine_id: None,
            name: "Image services".into(),
            home_id: firm.acme,
            access: None,
            arrangements: None,
            runtime_config: None,
            upstreams: vec![Upstream {
                protocol: "openai-responses".into(),
                base_url: "https://api.example.invalid".into(),
                options: json!({"auth":"none"}),
            }],
            api_key: None,
            secrets: None,
            key_machine_id: None,
            sources: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .provider;
    let catalogue = made.service_catalogue.as_ref().unwrap();
    let image = catalogue
        .iter()
        .find(|service| service.id == "image-api")
        .unwrap();
    assert!(
        image.available,
        "the keeping runtime supports the declared image codec"
    );
    assert_eq!(image.capabilities, ["image_generation"]);
    assert!(
        !image.enabled,
        "discovery does not opt a provider into paid services"
    );
    assert!(
        !catalogue
            .iter()
            .find(|service| service.id == "uninstalled-codec")
            .unwrap()
            .available
    );

    let id = made.base.id.to_string();
    let updated = world.app.call(ProviderUpdate, &[&id]).token(&firm.boss.token)
        .raw_body(&json!({"expected_revision":made.revision,"service_offerings":{"image-api":{"enabled":true}}}))
        .send().await.ok().into_full().unwrap().provider;
    let read = world
        .app
        .call(ProviderGet, &[&id])
        .token(&firm.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(read.service_offerings, updated.service_offerings);
    let image = read
        .service_catalogue
        .as_ref()
        .unwrap()
        .iter()
        .find(|service| service.id == "image-api")
        .unwrap();
    assert!(image.available && image.enabled);
}

#[tokio::test]
async fn declared_services_start_disabled_and_only_the_manager_can_store_explicit_choices() {
    let world = World::with_builtin_sources(Arc::new(sources())).await;
    let firm = resources::company(&world).await;
    let made = world
        .app
        .call(ProviderCreate, &[])
        .token(&firm.boss.token)
        .body(&CreateProvider::Api {
            runtime_machine_id: None,
            name: "Fixture services".into(),
            home_id: firm.acme,
            access: None,
            arrangements: None,
            runtime_config: None,
            upstreams: vec![Upstream {
                protocol: "openai-responses".into(),
                base_url: "https://api.example.invalid".into(),
                options: json!({"auth":"none"}),
            }],
            api_key: None,
            secrets: None,
            key_machine_id: None,
            sources: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .provider;
    let id = made.base.id.to_string();
    assert!(made.service_offerings.is_empty());
    assert_eq!(made.service_catalogue.as_ref().unwrap().len(), 1);
    assert_eq!(
        made.service_catalogue.as_ref().unwrap()[0].id,
        "fixture-search"
    );
    assert!(!made.service_catalogue.as_ref().unwrap()[0].enabled);
    assert!(made.service_catalogue.as_ref().unwrap()[0].available);
    assert_eq!(
        made.service_catalogue.as_ref().unwrap()[0].declared_groups,
        ["web"]
    );
    assert_eq!(
        made.service_catalogue.as_ref().unwrap()[0].catalogue_prices["search"].currency,
        "USD"
    );
    assert!(
        !serde_json::to_string(&made.service_catalogue)
            .unwrap()
            .contains("api.example.invalid")
    );
    let initial: Json = sqlx::query_scalar("SELECT service_offerings FROM providers WHERE id=$1")
        .bind(made.base.id)
        .fetch_one(&mut *world.conn().await)
        .await
        .unwrap();
    assert_eq!(initial, json!({}));
    let choices = json!({"fixture-search":{"enabled":true,"groups":["web"],"prices":{"search":{"amount":"2","currency":"credits","unit":"search"}},"future_option":{"kept":true}}});
    let body = json!({"expected_revision":made.revision,"service_offerings":choices});
    let denied = world
        .app
        .call(ProviderUpdate, &[&id])
        .token(&firm.viewer.token)
        .raw_body(&body)
        .send()
        .await;
    assert_eq!(denied.problem().code, ErrorCode::AuthzDenied);
    let updated = world
        .app
        .call(ProviderUpdate, &[&id])
        .token(&firm.boss.token)
        .raw_body(&body)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .provider;
    assert!(updated.revision > made.revision);
    assert!(updated.service_offerings["fixture-search"].enabled);
    assert!(updated.service_catalogue.as_ref().unwrap()[0].enabled);
    assert_eq!(
        updated.service_catalogue.as_ref().unwrap()[0].prices["search"].amount,
        "2"
    );
    assert_eq!(
        updated.service_catalogue.as_ref().unwrap()[0].prices["search"].currency,
        "credits"
    );
    assert_eq!(
        updated.service_catalogue.as_ref().unwrap()[0].catalogue_prices["search"].amount,
        "0.005"
    );
    let stored: Json = sqlx::query_scalar("SELECT service_offerings FROM providers WHERE id=$1")
        .bind(made.base.id)
        .fetch_one(&mut *world.conn().await)
        .await
        .unwrap();
    assert_eq!(
        stored["fixture-search"]["future_option"],
        json!({"kept":true})
    );
    let read = world
        .app
        .call(ProviderGet, &[&id])
        .token(&firm.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(read.service_offerings, updated.service_offerings);
    assert_eq!(read.service_catalogue, updated.service_catalogue);
    for invalid in [
        json!({"not-declared":{"enabled":true}}),
        json!({"fixture-search":{"enabled":true,"prices":{"search":{"amount":"-1","currency":"credits","unit":"search"}}}}),
        json!({"fixture-search":{"enabled":true,"prices":{"page":{"amount":"1","currency":"credits","unit":"page"}}}}),
    ] {
        let rejected = world
            .app
            .call(ProviderUpdate, &[&id])
            .token(&firm.boss.token)
            .raw_body(&json!({"expected_revision":updated.revision,"service_offerings":invalid}))
            .send()
            .await;
        assert_eq!(rejected.problem().code, ErrorCode::RequestInvalid);
    }
    let cleared = world
        .app
        .call(ProviderUpdate, &[&id])
        .token(&firm.boss.token)
        .raw_body(&json!({"expected_revision":updated.revision,"service_offerings":{}}))
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .provider;
    assert!(cleared.service_offerings.is_empty());
    assert!(!cleared.service_catalogue.as_ref().unwrap()[0].enabled);
    let final_value: Json =
        sqlx::query_scalar("SELECT service_offerings FROM providers WHERE id=$1")
            .bind(made.base.id)
            .fetch_one(&mut *world.conn().await)
            .await
            .unwrap();
    assert_eq!(final_value, json!({}));
}

use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::{Page, PageQuery};
use orch_api_contract::resources::{
    CapabilityBackendCandidate, CapabilityBackendQuery, ProviderCapabilityBackendList,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity::{self, NewApiToken};
use orch_provider_contract::capabilities::{Capability, Mode, Route};
use orch_types::{Id, new_id};

const CAPABILITY: &str = "fixture-capability";

async fn scene() -> (World, resources::Company, Id) {
    let mut declared = sources();
    declared.register_capabilities("backend-choice-fixture", Declarations {
        capabilities: BTreeMap::from([(CAPABILITY.into(), Capability {
            backend_setting: Some("agents.image_generation_backend".into()),
            mode_setting: Some("agents.image_generation".into()),
            ledger_unit: "search".into(),
            modes: BTreeMap::from([("service".into(), Mode { routes: vec!["fixture.api".into()], ..Mode::default() })]),
            ..Capability::default()
        })]),
        routes: BTreeMap::from([("fixture.api".into(), Route {
            executor: Some("api-backed".into()), uses_backend: true,
            capabilities: vec![CAPABILITY.into()], title: "Fixture service".into(),
            text: BTreeMap::from([("en".into(),"Fixture service".into()),("zh-CN".into(),"测试服务".into())]),
            ..Route::default()
        })]),
        endpoint_rules: vec![EndpointRule {
            when: json!({"required":["host"],"properties":{"host":{"const":"api.example.invalid"}}}),
            endpoints: BTreeMap::from([("fixture-backends".into(),Endpoint { address:"https://api.example.invalid".into(),interface:"fixture-search".into(),..Endpoint::default() })]),
            offerings: ["a", "b", "c"].into_iter().map(|id|(format!("fixture-{id}"), Offering {
                title:format!("Choice {id}"), endpoint:"fixture-backends".into(),kind:"service".into(),
                capabilities: BTreeMap::from([(CAPABILITY.into(),Support { enabled:true,..Support::default() })]),
                ..Offering::default()
            })).collect(),
            ..EndpointRule::default()
        }],
        ..Declarations::default()
    }).unwrap();
    let world = World::with_builtin_sources(Arc::new(declared)).await;
    let firm = resources::company(&world).await;
    let made = world
        .app
        .call(ProviderCreate, &[])
        .token(&firm.boss.token)
        .body(&CreateProvider::Api {
            runtime_machine_id: None,
            name: "Backend choices".into(),
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
    let machine =
        resources::machine_forwarding_to(&world, firm.boss.id(), &["https://api.example.invalid"])
            .await;
    sqlx::query("UPDATE providers SET service_offerings=$2,key_machine_id=$3,status='ready' WHERE id=$1")
        .bind(made.base.id)
        .bind(json!({"fixture-a":{"enabled":true},"fixture-b":{"enabled":true},"fixture-c":{"enabled":true}}))
        .bind(machine).execute(&mut *world.conn().await).await.unwrap();
    (world, firm, made.base.id)
}

fn query(place: Id) -> CapabilityBackendQuery {
    CapabilityBackendQuery {
        capability: CAPABILITY.into(),
        place,
        q: None,
        offering: None,
        page: PageQuery {
            limit: Some(1),
            cursor: None,
        },
    }
}
async fn choices(
    world: &World,
    token: &str,
    provider: Id,
    query: CapabilityBackendQuery,
) -> Page<CapabilityBackendCandidate> {
    world
        .app
        .call(ProviderCapabilityBackendList, &[&provider])
        .token(token)
        .query(&query)
        .send()
        .await
        .ok()
}
fn rule(kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":"ProviderCapabilityBackendList","actions":["read"]}]})
}
async fn key(world: &World, user: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Capability backend read",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}

#[tokio::test]
async fn backend_choices_are_paged_searchable_and_do_not_disclose_private_configuration() {
    let (world, firm, provider) = scene().await;
    let mut q = query(firm.acme);
    let mut found = Vec::new();
    loop {
        let page = choices(&world, &firm.boss.token, provider, q.clone()).await;
        assert_eq!(page.items.len(), 1);
        assert!(
            page.items[0].eligible,
            "the fixture has ordinary Use and a declared enabled offering"
        );
        let reference = page.items[0].backend.offering.as_ref().unwrap();
        assert_eq!(reference.provider, provider);
        found.push(reference.offering.clone());
        let raw = serde_json::to_string(&page).unwrap();
        assert!(
            !raw.contains("api.example.invalid")
                && !raw.contains("api_key")
                && !raw.contains("credential")
        );
        assert_eq!(page.items[0].route_text["zh-CN"], "测试服务");
        q.page.cursor = page.next_cursor;
        if q.page.cursor.is_none() {
            break;
        }
        assert!(found.len() < 4, "a bounded cursor must advance");
    }
    assert_eq!(found, ["fixture-a", "fixture-b", "fixture-c"]);
    let mut q = query(firm.acme);
    q.q = Some("Choice c".into());
    let page = choices(&world, &firm.boss.token, provider, q).await;
    assert_eq!(page.items[0].offering_name, "Choice c");
    assert!(page.next_cursor.is_none(), "search precedes the page limit");
    let mut q = query(firm.acme);
    q.page.cursor = choices(&world, &firm.boss.token, provider, q.clone())
        .await
        .next_cursor;
    q.q = Some("changed query".into());
    let refused = world
        .app
        .call(ProviderCapabilityBackendList, &[&provider])
        .token(&firm.boss.token)
        .query(&q)
        .send()
        .await;
    assert_eq!(refused.problem().code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn backend_choices_require_both_real_read_sources_and_never_turn_view_into_use() {
    let (world, firm, provider) = scene().await;
    let ordinary = choices(&world, &firm.boss.token, provider, query(firm.acme)).await;
    let full = key(
        &world,
        firm.boss.id(),
        vec![rule("provider", provider), rule("org", firm.acme)],
    )
    .await;
    assert_eq!(
        choices(&world, &full, provider, query(firm.acme)).await,
        ordinary
    );
    for rules in [
        vec![rule("provider", provider)],
        vec![rule("org", firm.acme)],
        vec![rule("provider", new_id()), rule("org", firm.acme)],
    ] {
        let token = key(&world, firm.boss.id(), rules).await;
        let refused = world
            .app
            .call(ProviderCapabilityBackendList, &[&provider])
            .token(&token)
            .query(&query(firm.acme))
            .send()
            .await;
        assert_eq!(refused.problem().code, ErrorCode::AuthzDenied);
    }
    resources::name_person(
        &world,
        "provider",
        provider,
        "provider.view",
        firm.viewer.id(),
    )
    .await;
    let view = choices(&world, &firm.viewer.token, provider, query(firm.acme)).await;
    assert!(!view.items[0].eligible);
    assert!(
        view.items[0].reason.is_some(),
        "a visible but unusable candidate has a reason"
    );
    let outside = key(
        &world,
        firm.stranger.id(),
        vec![rule("provider", provider), rule("org", firm.acme)],
    )
    .await;
    let refused = world
        .app
        .call(ProviderCapabilityBackendList, &[&provider])
        .token(&outside)
        .query(&query(firm.acme))
        .send()
        .await;
    assert_eq!(
        refused.problem().code,
        ErrorCode::AuthzDenied,
        "a key never manufactures ordinary View"
    );
    sqlx::query("UPDATE providers SET service_offerings=jsonb_set(service_offerings,'{fixture-c,enabled}','false') WHERE id=$1")
        .bind(provider).execute(&mut *world.conn().await).await.unwrap();
    let mut selected = query(firm.acme);
    selected.offering = Some("fixture-c".into());
    let selected = choices(&world, &firm.boss.token, provider, selected).await;
    assert_eq!(selected.items.len(), 1);
    assert_eq!(selected.items[0].offering_name, "Choice c");
    assert!(
        !selected.items[0].eligible,
        "an old value is named but cannot be newly admitted"
    );
}

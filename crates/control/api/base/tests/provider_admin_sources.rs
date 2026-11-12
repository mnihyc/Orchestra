//! Administrative source credentials use the native manager/revision/sealing path, never the model key.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]
mod places;
mod resources;
mod support;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{
    CreateProvider, ProviderCreate, ProviderGet, ProviderSourcesSet, Upstream,
};
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_db::secrets::MasterKey;
use orch_provider_contract::builtins::{BuiltinSource, BuiltinSources, VendorClient};
use orch_provider_contract::source::{AdminCredential, SourceDescriptorFacts};
use orch_provider_contract::{
    FactMember, FactsReport, FactsSource, SourceAsk, SourceDescriptor, SourceError,
};
use orch_types::Clock;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use support::World;
fn declared() -> SourceDescriptor {
    let mut descriptor = SourceDescriptor::new(
        "fixture-admin",
        "Fixture source",
        "A native source with a separate admin credential.",
    );
    descriptor.facts = Some(SourceDescriptorFacts {
        reports: vec![FactMember::Spend],
        ..Default::default()
    });
    descriptor.set_admin_credential(AdminCredential {
        labels: BTreeMap::from([
            ("en".into(), "Admin key".into()),
            ("zh-CN".into(), "管理密钥".into()),
        ]),
        reports: vec![FactMember::Spend],
    });
    descriptor
}
fn next_declared() -> SourceDescriptor {
    let mut value = declared();
    value.name = "fixture-next".into();
    value
}
struct Source(&'static str);
#[async_trait::async_trait]
impl FactsSource for Source {
    fn describe(&self) -> SourceDescriptor {
        let mut descriptor = declared();
        descriptor.name = self.0.into();
        descriptor
    }
    async fn read_facts(&self, _ask: SourceAsk) -> Result<FactsReport, SourceError> {
        Ok(FactsReport(json!({"spend":[]})))
    }
}
fn make(_client: Arc<dyn VendorClient>, _clock: Arc<dyn Clock>) -> Arc<dyn FactsSource> {
    Arc::new(Source("fixture-admin"))
}
fn make_next(_client: Arc<dyn VendorClient>, _clock: Arc<dyn Clock>) -> Arc<dyn FactsSource> {
    Arc::new(Source("fixture-next"))
}
async fn fixture() -> (World, resources::Company, orch_types::Id) {
    install_master_key(MasterKey::new([7; 32]));
    let table = BuiltinSources::new([
        BuiltinSource {
            name: "fixture-admin",
            hosts: &["api.example.com"],
            descriptor: declared,
            matches: |url| url.host_str() == Some("api.example.com"),
            make,
        },
        BuiltinSource {
            name: "fixture-next",
            hosts: &["other.example.com"],
            descriptor: next_declared,
            matches: |url| url.host_str() == Some("other.example.com"),
            make: make_next,
        },
    ])
    .unwrap();
    let world = World::with_builtin_sources(Arc::new(table)).await;
    let c = resources::company(&world).await;
    let body = CreateProvider::Api {
        runtime_machine_id: None,
        key_machine_id: None,
        name: "Admin-source fixture".into(),
        home_id: c.acme,
        access: None,
        arrangements: None,
        runtime_config: None,
        upstreams: vec![Upstream {
            protocol: "openai-responses".into(),
            base_url: "https://api.example.com/v1".into(),
            options: json!({}),
        }],
        api_key: Some("ordinary-model-fixture".into()),
        secrets: None,
        sources: None,
    };
    let made = world
        .app
        .call(ProviderCreate, &[])
        .token(&c.boss.token)
        .body(&body)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .provider;
    sqlx::query("UPDATE providers SET status='disabled' WHERE id=$1")
        .bind(made.base.id)
        .execute(&world.db.pool)
        .await
        .unwrap();
    (world, c, made.base.id)
}
async fn key_bytes(world: &World, id: orch_types::Id) -> Vec<u8> {
    sqlx::query_scalar(
        "SELECT sealed FROM secrets WHERE owner_kind='provider' AND owner_id=$1 AND name='api_key'",
    )
    .bind(id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap()
}
#[tokio::test]
async fn a_default_sources_admin_key_is_sealed_replaced_and_removed_without_touching_the_model_key()
{
    let (world, c, id) = fixture().await;
    let before = world
        .app
        .call(ProviderGet, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(
        serde_json::to_value(before.facts_state.spend).unwrap(),
        json!({"unsupported_because":{"reason":"needs_admin_key"}})
    );
    assert_eq!(
        serde_json::to_value(before.facts_state.balances).unwrap(),
        json!("unsupported")
    );
    let primary = key_bytes(&world, id).await;
    let mut revision = before.revision;
    for admin in [
        Some("admin-first-fixture"),
        Some("admin-second-fixture"),
        None,
    ] {
        let response=world.app.call(ProviderSourcesSet,&[&id]).token(&c.boss.token).raw_body(&json!({"expected_revision":revision,"facts":{"by":"default","secrets":{"admin_key":admin}}})).send().await;
        let saved = response.ok();
        let rendered = serde_json::to_string(&saved).unwrap();
        assert!(
            !rendered.contains("admin-first-fixture")
                && !rendered.contains("admin-second-fixture")
                && !rendered.contains("ordinary-model-fixture")
        );
        revision = saved.revision;
        assert_eq!(
            saved
                .sources
                .facts
                .secrets
                .iter()
                .any(|name| name == "admin_key"),
            admin.is_some()
        );
        assert_eq!(
            key_bytes(&world, id).await,
            primary,
            "a source credential is independent of the model key"
        );
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM secrets WHERE owner_kind='provider' AND owner_id=$1 AND name='source:facts:field:admin_key'").bind(id).fetch_one(&world.db.pool).await.unwrap();
        assert_eq!(count, if admin.is_some() { 1 } else { 0 });
        if admin.is_none() {
            assert_eq!(
                serde_json::to_value(saved.facts_state.spend).unwrap(),
                json!({"unsupported_because":{"reason":"needs_admin_key"}})
            );
        }
    }
}
#[tokio::test]
async fn an_admin_source_key_requires_current_management_and_the_reviewed_revision() {
    let (world, c, id) = fixture().await;
    resources::name_person(&world, "provider", id, "provider.view", c.viewer.id()).await;
    let provider = world
        .app
        .call(ProviderGet, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    let body = json!({"expected_revision":provider.revision,"facts":{"by":"default","secrets":{"admin_key":"private-admin-fixture"}}});
    let denied = world
        .app
        .call(ProviderSourcesSet, &[&id])
        .token(&c.viewer.token)
        .raw_body(&body)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    let saved = world
        .app
        .call(ProviderSourcesSet, &[&id])
        .token(&c.boss.token)
        .raw_body(&body)
        .send()
        .await
        .ok();
    let stale = world
        .app
        .call(ProviderSourcesSet, &[&id])
        .token(&c.boss.token)
        .raw_body(&body)
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    let visible = world
        .app
        .call(ProviderGet, &[&id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert!(visible.sources.facts.secrets.is_empty());
    assert!(
        !serde_json::to_string(&visible)
            .unwrap()
            .contains("private-admin-fixture")
    );
    assert_eq!(saved.sources.facts.secrets, ["admin_key"]);
}

#[tokio::test]
async fn changing_the_default_recipient_does_not_lend_it_the_previous_sources_admin_key() {
    let (world, c, id) = fixture().await;
    let before = world
        .app
        .call(ProviderGet, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    world.app.call(ProviderSourcesSet,&[&id]).token(&c.boss.token).raw_body(&json!({"expected_revision":before.revision,"facts":{"by":"default","secrets":{"admin_key":"belongs-only-to-first-source"}}})).send().await.ok();
    sqlx::query("UPDATE providers SET upstreams=$2 WHERE id=$1").bind(id).bind(json!([{"protocol":"openai-responses","base_url":"https://other.example.com/v1","options":{}}])).execute(&world.db.pool).await.unwrap();
    let changed = world
        .app
        .call(ProviderGet, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(changed.sources.facts.name.as_deref(), Some("fixture-next"));
    assert!(changed.sources.facts.secrets.is_empty());
    assert_eq!(
        serde_json::to_value(changed.facts_state.spend).unwrap(),
        json!({"unsupported_because":{"reason":"needs_admin_key"}})
    );
}

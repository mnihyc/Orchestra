//! Permission pages show the binding and defaults that work uses, with bounded giving and entitled deciders.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

#[path = "../../authz/tests/support/mod.rs"]
mod access_world;

use access_world::World;
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission as P;
use orch_api_core::routes::access::page::permission_page;
use orch_authz::pages::Target;
use orch_authz::{Caller, requests};
use orch_types::{Id, Json};
use serde_json::json;

async fn setting(w: &World, node: Id, key: &str, value: Json) {
    sqlx::query("INSERT INTO settings(node_id,key,value) VALUES($1,$2,$3)")
        .bind(node)
        .bind(key)
        .bind(value)
        .execute(&mut *w.conn().await)
        .await
        .unwrap();
}

async fn page(w: &World, who: Id, target: Target) -> orch_api_contract::access::PermissionPage {
    let principal = w.principal(Caller::session(who)).await;
    let read = w.read(who, target).await;
    permission_page(&mut *w.conn().await, &principal, &read, Some(w.root), w.now)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_thread_page_names_its_actual_provider_model_and_effort_and_a_page_manager_cannot_give_an_unheld_model()
 {
    let w = World::new().await;
    let provider = w.provider(w.maker, w.root, "Team provider").await;
    w.model(provider, "chosen-model", &["high"]).await;
    sqlx::query("UPDATE threads SET provider_id = $2 WHERE node_id = $1")
        .bind(w.thread)
        .bind(provider)
        .execute(&mut *w.conn().await)
        .await
        .unwrap();
    setting(&w, w.thread, "agent.model", json!("chosen-model")).await;
    setting(&w, w.thread, "agent.effort", json!("high")).await;
    let shown = page(&w, w.boss, Target::Live(w.thread)).await;
    let dependency = shown
        .depends
        .iter()
        .find(|each| each.object.id == provider)
        .unwrap();
    assert_eq!(dependency.for_permission, "thread.interact");
    assert_eq!(dependency.permission, "provider.use");
    assert_eq!(
        dependency.options.as_ref().unwrap()["models"],
        json!(["chosen-model"])
    );
    assert_eq!(
        dependency.options.as_ref().unwrap()["efforts"],
        json!(["high"])
    );
    assert!(
        !dependency.can_give,
        "administering the provider page grants no model use"
    );
    assert_eq!(dependency.object.owner.as_ref().unwrap().id, w.maker);
    let owned = page(&w, w.maker, Target::Live(w.thread)).await;
    assert!(
        owned
            .depends
            .iter()
            .any(|each| each.object.id == provider && each.can_give)
    );

    let manager = w.member("model-manager").await;
    w.grant(
        "provider",
        provider,
        P::AccessManage,
        Holder::User { id: manager },
    )
    .await;
    w.put(
        "provider",
        provider,
        P::ProviderUse,
        Holder::User { id: manager },
        json!({"models":["another-model"],"efforts":["low"]}),
    )
    .await;
    let principal = w.principal(Caller::session(w.boss)).await;
    let options = orch_access_contract::decisions::MissingOptions {
        models: Some(vec!["chosen-model".into()]),
        efforts: Some(vec!["high".into()]),
    };
    let givers = requests::givers_for(
        &mut *w.conn().await,
        &principal,
        access_world::things::object("provider", provider),
        &[P::ProviderUse],
        Some(&options),
        &orch_authz::usable::ResourcePolicy::default(),
    )
    .await
    .unwrap();
    assert!(!givers.nearest.contains(&manager) && !givers.further.contains(&manager));
}

#[tokio::test]
async fn a_places_dependencies_follow_its_inherited_defaults_and_its_private_default_environment() {
    let w = World::new().await;
    let provider = w.provider(w.maker, w.root, "Default provider").await;
    let profile = w.thing("agent", w.maker, w.root, "Default profile").await;
    let template = w.thing("template", w.maker, w.root, "Default recipe").await;
    let environment = w
        .thing("environment", w.maker, w.topic, "Private main")
        .await;
    setting(&w, w.project, "agent.default_provider_id", json!(provider)).await;
    setting(&w, w.project, "agent.default_agent_id", json!(profile)).await;
    setting(&w, w.project, "env.template_id", json!(template)).await;
    setting(&w, w.project, "agent.model", json!("default-model")).await;
    setting(&w, w.project, "agent.effort", json!("high")).await;
    sqlx::query("INSERT INTO settings (node_id,key,value) VALUES ($1,'env.default_id',to_jsonb($2::uuid::text)) ON CONFLICT (node_id,key) DO UPDATE SET value=excluded.value")
        .bind(w.topic).bind(environment).execute(&mut *w.conn().await).await.unwrap();
    let shown = page(&w, w.boss, Target::Live(w.topic)).await;
    assert!(shown.depends.iter().any(|each| each.object.id == provider
        && each.options.as_ref().unwrap()["models"] == json!(["default-model"])));
    assert!(
        shown
            .depends
            .iter()
            .any(|each| each.object.id == profile && each.permission == "agent.use")
    );
    assert!(
        shown
            .depends
            .iter()
            .any(|each| each.object.id == environment
                && each.permission == "environment.manage"
                && !each.can_give)
    );
    assert!(
        !shown.depends.iter().any(|each| each.object.id == template),
        "a default environment replaces new-unit dependencies"
    );
    setting(&w, w.topic, "env.thread_default", json!("fresh")).await;
    let fresh = page(&w, w.boss, Target::Live(w.topic)).await;
    assert!(
        !fresh
            .depends
            .iter()
            .any(|each| each.object.id == environment),
        "fresh work does not manage the shared default environment"
    );
    assert!(
        fresh
            .depends
            .iter()
            .any(|each| each.object.id == template && each.permission == "template.use")
    );
}

#[tokio::test]
async fn a_subscriptions_page_names_the_profile_its_harness_uses() {
    let w = World::new().await;
    let provider = w.provider(w.maker, w.root, "Subscription").await;
    let profile = w
        .thing("agent", w.maker, w.root, "Subscription profile")
        .await;
    sqlx::query("UPDATE providers SET kind = 'subscription', agent_id = $2, harness_kind = 'codex' WHERE id = $1")
        .bind(provider).bind(profile).execute(&mut *w.conn().await).await.unwrap();
    let shown = page(
        &w,
        w.maker,
        Target::Resource(access_world::things::object("provider", provider)),
    )
    .await;
    assert!(
        shown
            .depends
            .iter()
            .any(|each| each.for_permission == "provider.use"
                && each.object.id == profile
                && each.permission == "agent.use")
    );
}

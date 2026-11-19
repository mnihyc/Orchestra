//! Provider address policies refuse a new binding and a rebind with the current provider and controlling scope.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::binding::{
    EnvironmentChoice, ThreadBindingOptionsGet, ThreadBindingSet, ThreadBindingSetBody,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, ThreadCreate};
use orch_settings::Target;
use orch_settings_contract::registry::keys;
use rig::{MODEL, Scene};
use serde_json::json;

#[tokio::test]
async fn owning_a_provider_does_not_bypass_the_destinations_address_policy_at_creation_or_rebinding()
 {
    let s = Scene::new().await;
    let existing = s.thread().await;
    let project = s.tree.project.base.id;
    s.rig
        .world
        .set_at(Target::Node(project), keys::PROVIDERS_ALLOWED, json!([]))
        .await;
    let options = s
        .rig
        .world
        .app
        .call(ThreadBindingOptionsGet, &[&existing.base.id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let choice = options
        .providers
        .iter()
        .find(|choice| choice.provider.id == s.on.provider)
        .unwrap();
    assert!(!choice.usable);
    assert_eq!(choice.reason.as_deref(), Some("provider_not_allowed"));
    assert_eq!(
        serde_json::to_value(&choice.policy_scope).unwrap()["id"],
        json!(project)
    );
    let created = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&CreateThread {
            name: Some("Forbidden destination".to_owned()),
            binding: Some(CreateThreadBinding {
                agent_id: s.on.agent,
                provider_id: s.on.provider,
                arrangement: None,
            }),
            model: Some(MODEL.to_owned()),
            environment: Some(EnvironmentChoice::Use(s.on.environment)),
            ..CreateThread::default()
        })
        .send()
        .await
        .problem();
    let rebound = s
        .rig
        .world
        .app
        .call(ThreadBindingSet, &[&existing.base.id])
        .token(&s.ada.token)
        .body(&ThreadBindingSetBody {
            agent_id: s.on.agent,
            provider_id: s.on.provider,
            arrangement: None,
            model: Some(MODEL.to_owned()),
            effort: None,
        })
        .send()
        .await
        .problem();
    for refused in [created, rebound] {
        assert_eq!(refused.code, ErrorCode::ProviderNotAllowed);
        assert_eq!(refused.extra["provider"]["id"], json!(s.on.provider));
        assert_eq!(refused.extra["scope"]["node"], json!(project));
    }
}

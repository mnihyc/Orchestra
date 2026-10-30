//! Compact choices report applicable caps and anonymous activity without changing resource access.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod rig;
mod support;
mod usage_world;

use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::access::NewResourceAccess;
use orch_api_contract::environment::{EnvironmentGet, EnvironmentList, EnvironmentListQuery};
use orch_api_contract::resources::{CreateProvider, ProviderCreate, ProviderGet, Upstream};
use orch_api_contract::settings::ThreadParameterGet;
use orch_api_contract::usage::{CapInput, CapMeter, CapSplit, CapSubjectInput, UsageCapCreate};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::resources::access_fixtures;
use orch_types::{Id, Time, new_id};
use serde_json::json;
use support::World;
use usage_world::{Use, another_thread, spend, team};

fn thread_content(node: orch_api_contract::nodes::Node) -> orch_api_contract::threads::Thread {
    match node {
        orch_api_contract::nodes::Node::Thread(thread) => thread,
        _ => panic!("the fixture must read a thread"),
    }
}

#[tokio::test]
async fn a_resource_switch_preview_has_thread_status_only_for_readable_bound_threads() {
    use orch_api_contract::resource_facts::{FactTarget, ResourceImpactRead};
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await;
    let target = FactTarget {
        object: ObjectRef {
            kind: ResourceKind::Provider.into(),
            id: scene.on.provider,
        },
        model_id: None,
    };
    let owner = scene
        .rig
        .world
        .app
        .call(ResourceImpactRead, &[])
        .token(&scene.ada.token)
        .body(&target)
        .send()
        .await
        .ok();
    assert_eq!(
        owner.thread_displays[&thread.base.id].state,
        thread.display.state
    );
    scene
        .rig
        .lets_use(
            "provider",
            scene.on.provider,
            "provider.view",
            &scene.bea,
            json!({}),
        )
        .await;
    let outsider = scene
        .rig
        .world
        .app
        .call(ResourceImpactRead, &[])
        .token(&scene.bea.token)
        .body(&target)
        .send()
        .await
        .ok();
    assert!(
        outsider.thread_displays.is_empty(),
        "provider View does not grant a bound thread's status"
    );
}

#[tokio::test]
async fn thread_status_costs_use_recorded_counted_credits_and_keep_missing_prices_unknown() {
    use orch_api_contract::nodes::NodeGet;
    let world = World::new().await;
    let scene = team(&world).await;
    let empty = thread_content(
        world
            .app
            .call(NodeGet, &[&scene.thread])
            .token(&scene.boss.token)
            .send()
            .await
            .ok(),
    );
    assert!(empty.costs.is_none());
    spend(&world, &scene, &Use::of(&world, scene.boss.id(), 100)).await;
    let unrated = thread_content(
        world
            .app
            .call(NodeGet, &[&scene.thread])
            .token(&scene.boss.token)
            .send()
            .await
            .ok(),
    )
    .costs
    .unwrap();
    assert!(
        unrated.total.credits.is_none(),
        "a missing price is never zero"
    );
    assert_eq!(unrated.total.unrated_tokens, 100);
    spend(
        &world,
        &scene,
        &Use::of(&world, scene.boss.id(), 200).rated("1.25", "provider"),
    )
    .await;
    let current = spend(
        &world,
        &scene,
        &Use::of(&world, scene.boss.id(), 40).rated("0.25", "catalogue"),
    )
    .await;
    spend(
        &world,
        &scene,
        &Use::of(&world, scene.boss.id(), 500)
            .rated("9", "provider")
            .trace(),
    )
    .await;
    let costs = thread_content(
        world
            .app
            .call(NodeGet, &[&scene.thread])
            .token(&scene.boss.token)
            .send()
            .await
            .ok(),
    )
    .costs
    .unwrap();
    assert_eq!(costs.total.credits.unwrap().parse::<f64>().unwrap(), 1.5);
    assert_eq!(costs.total.estimated_credits.parse::<f64>().unwrap(), 0.25);
    assert_eq!(costs.total.unrated_tokens, 100);
    let mut conn = world.conn().await;
    let current = orch_db::usage::ledger::status_credits(&mut conn, scene.thread, Some(current))
        .await
        .unwrap();
    assert_eq!(current.turn_credits.unwrap().parse::<f64>().unwrap(), 0.25);
    assert_eq!(current.turn_unrated_tokens, 0);
}

fn concurrent(provider: Id, subject: CapSubjectInput, amount: &str) -> CapInput {
    let split = if matches!(&subject, CapSubjectInput::User { .. }) {
        CapSplit::Shared
    } else {
        CapSplit::Each
    };
    CapInput {
        node_id: None,
        provider_id: Some(provider),
        subject,
        split,
        models: None,
        tiers: None,
        harness_kinds: None,
        meter: CapMeter::Running,
        period: None,
        counted_from: None,
        amount: Some(amount.into()),
        at_limit: None,
        warn_percent: None,
        uncounted: None,
        note: None,
    }
}

#[tokio::test]
async fn reading_a_thread_does_not_grant_its_providers_detail_facts() {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    let owner = scene
        .rig
        .world
        .app
        .call(ThreadParameterGet, &[&thread])
        .token(&scene.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(owner.provider.as_ref().unwrap().base.id, scene.on.provider);
    assert!(owner.provider.as_ref().unwrap().choice_usage.is_some());
    let profile = scene
        .rig
        .world
        .app
        .call(orch_api_contract::resources::AgentGet, &[&scene.on.agent])
        .token(&scene.ada.token)
        .send()
        .await
        .ok();
    assert!(
        profile
            .providers
            .as_ref()
            .unwrap()
            .iter()
            .any(|provider| provider.id == scene.on.provider)
    );
    assert!(
        profile
            .models
            .as_ref()
            .unwrap()
            .iter()
            .any(|model| model.provider_id == scene.on.provider
                && model.model.model_name == rig::MODEL)
    );
    scene
        .rig
        .lets_use("agent", scene.on.agent, "agent.view", &scene.cai, json!({}))
        .await;
    let private_profile = scene
        .rig
        .world
        .app
        .call(orch_api_contract::resources::AgentGet, &[&scene.on.agent])
        .token(&scene.cai.token)
        .send()
        .await
        .ok();
    assert!(private_profile.providers.unwrap().is_empty());
    assert!(private_profile.models.unwrap().is_empty());
    let denied = scene
        .rig
        .world
        .app
        .call(ProviderGet, &[&scene.on.provider])
        .token(&scene.cai.token)
        .send()
        .await
        .problem();
    assert_eq!(
        denied.code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
    let reader = scene
        .rig
        .world
        .app
        .call(ThreadParameterGet, &[&thread])
        .token(&scene.cai.token)
        .send()
        .await
        .ok();
    assert!(!reader.model.writable);
    assert!(reader.provider.is_none());
}

#[tokio::test]
async fn provider_choices_keep_the_callers_cap_precedence_and_the_existing_performance_span() {
    let world = World::new().await;
    let team = team(&world).await;
    for body in [
        concurrent(team.provider, CapSubjectInput::Everyone, "5"),
        concurrent(
            team.provider,
            CapSubjectInput::User { id: team.boss.id() },
            "2",
        ),
        concurrent(
            team.provider,
            CapSubjectInput::User {
                id: team.viewer.id(),
            },
            "1",
        ),
    ] {
        world
            .app
            .call(UsageCapCreate, &[])
            .token(&team.boss.token)
            .body(&body)
            .send()
            .await
            .ok();
    }
    spend(&world, &team, &Use::of(&world, team.boss.id(), 100)).await;
    // A known completed stream is a histogram observation, never an inferred zero speed.
    sqlx::query("UPDATE usage_daily SET requests=20, speed_buckets='{\"80\":20}', first_token_buckets='{\"120\":20}' WHERE provider_id=$1")
        .bind(team.provider).execute(&world.db.pool).await.unwrap();
    let provider = world
        .app
        .call(ProviderGet, &[&team.provider])
        .token(&team.boss.token)
        .send()
        .await
        .ok();
    let usage = provider.choice_usage.unwrap();
    assert_eq!(usage.observed_at, Time::new(world.now()));
    assert_eq!(usage.caps.len(), 1);
    assert_eq!(usage.caps[0].cap.amount.as_deref(), Some("2"));
    assert_eq!(usage.caps[0].person.as_ref().unwrap().id, team.boss.id());
    assert_eq!(usage.performance.len(), 1);
    assert_eq!(usage.performance[0].model, "gpt-6-sol");
    assert_eq!(usage.performance[0].performance.speed_samples, 20);
    assert!(usage.performance[0].performance.speed_milli.is_some());
    assert!(usage.caps.iter().all(|cap| cap.replaced_by.is_none()));
}

#[tokio::test]
async fn administering_a_private_provider_does_not_disclose_its_choice_usage() {
    let world = World::new().await;
    let team = team(&world).await;
    let provider = world
        .app
        .call(ProviderCreate, &[])
        .token(&team.boss.token)
        .body(&CreateProvider::Api {
            runtime_machine_id: None,
            name: "Private key".into(),
            home_id: team.acme,
            access: Some(NewResourceAccess::OnlyMe),
            arrangements: None,
            runtime_config: None,
            upstreams: vec![Upstream {
                protocol: "openai-responses".into(),
                base_url: "https://model.example/v1".into(),
                options: json!({}),
            }],
            api_key: Some("synthetic-test-key".into()),
            secrets: None,
            key_machine_id: None,
            sources: None,
        })
        .send()
        .await
        .ok()
        .provider;
    assert!(provider.choice_usage.is_some());
    let administrative = world
        .app
        .call(ProviderGet, &[&provider.base.id])
        .token(&team.admin.token)
        .send()
        .await
        .problem();
    assert_eq!(
        administrative.code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn environment_lists_include_observed_zeroes_and_anonymous_activity_counts() {
    let world = World::new().await;
    let team = team(&world).await;
    let mut ids = Vec::new();
    for name in ["Active", "Quiet"] {
        let mut conn = world.conn().await;
        let id = access_fixtures::insert_environment(
            &mut conn,
            team.boss.id(),
            Some(team.acme),
            name,
            "managed",
        )
        .await
        .unwrap();
        start_resource(
            &mut conn,
            ObjectRef {
                kind: ResourceKind::Environment.into(),
                id,
            },
            team.boss.id(),
            NewAccess::OnlyMe,
            Time::new(world.now()),
        )
        .await
        .unwrap();
        ids.push(id);
    }
    let idle = another_thread(&world, &team, "Idle").await;
    let blocked = another_thread(&world, &team, "Blocked").await;
    sqlx::query("UPDATE threads SET environment_id=$1 WHERE node_id=ANY($2)")
        .bind(ids[0])
        .bind([team.thread, idle, blocked])
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET state='running' WHERE node_id=$1")
        .bind(team.thread)
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET state='blocked',blocked_reason='environment_unavailable' WHERE node_id=$1")
        .bind(blocked)
        .execute(&world.db.pool)
        .await
        .unwrap();
    for state in ["open", "closed"] {
        sqlx::query("INSERT INTO terminal_sessions(id,environment_id,env_generation,opened_by,shell,cwd,cols,rows,state) VALUES($1,$2,1,$3,'/bin/sh','/',80,24,$4)")
            .bind(new_id()).bind(ids[0]).bind(team.boss.id()).bind(state).execute(&world.db.pool).await.unwrap();
    }
    let listed = world
        .app
        .call(EnvironmentList, &[])
        .token(&team.boss.token)
        .query(&EnvironmentListQuery::default())
        .send()
        .await
        .ok();
    let active = listed
        .iter()
        .filter_map(orch_api_contract::common::Listed::as_visible)
        .find(|environment| environment.base.id == ids[0])
        .unwrap();
    let activity = active.base.activity.as_ref().unwrap();
    assert_eq!(
        (
            activity.running_threads,
            activity.idle_threads,
            activity.blocked_threads
        ),
        (1, 1, 1)
    );
    assert_eq!((activity.active_turns, activity.open_shells), (0, 1));
    let quiet = listed
        .iter()
        .filter_map(orch_api_contract::common::Listed::as_visible)
        .find(|environment| environment.base.id == ids[1])
        .unwrap()
        .base
        .activity
        .as_ref()
        .unwrap();
    assert_eq!(
        (
            quiet.running_threads,
            quiet.idle_threads,
            quiet.blocked_threads,
            quiet.active_turns,
            quiet.open_shells
        ),
        (0, 0, 0, 0, 0)
    );
    let denied = world
        .app
        .call(EnvironmentGet, &[&ids[0]])
        .token(&team.stranger.token)
        .send()
        .await
        .problem();
    assert_eq!(
        denied.code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
}

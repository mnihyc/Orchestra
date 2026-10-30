//! Binding choices before the first thread, real machine readiness, and agreement between offered and requested placement.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod binding_support;
mod places;
mod rig;
mod support;

use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::binding::{
    ArrangementType, BindingOptions, EnvironmentChoice, HarnessPlace, NodeBindingOptionsGet,
    NodeBindingOptionsGetQuery,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, ThreadCreate};
use orch_api_contract::resources::{MachineAgentCreate, MachineAgentCreateBody};
use orch_authz::ownership::{self, NewAccess};
use orch_types::{Id, Time};
use rig::Scene;
use serde_json::{Value, json};

fn reported() -> Value {
    json!({
        "adapters": [{"kind": "fake", "adapter_version": 1, "builtin": true}],
        "environment_descriptor": {"lifecycle": "attach"},
        "harnesses": [{"kind": "fake", "path": "/test/harness", "version": "1.0.0", "daemon": false}],
        "policy": {"agent.harnesses": [{"kind": "fake", "path": "/test/harness"}], "provider.arrangements": ["separated", "key_separated"]}
    })
}

async fn facts(s: &Scene, machine: Id, value: Value) {
    sqlx::query("UPDATE machines SET facts = $2, owner_id = $3 WHERE id = $1")
        .bind(machine)
        .bind(value)
        .bind(s.ada.id())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
}

async fn options(s: &Scene, node: Id, environment: Option<Id>) -> BindingOptions {
    s.rig
        .world
        .app
        .call(NodeBindingOptionsGet, &[&node])
        .token(&s.ada.token)
        .query(&NodeBindingOptionsGetQuery {
            environment_id: environment,
            ..Default::default()
        })
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn a_topic_and_a_folder_can_show_binding_choices_without_making_a_thread() {
    let s = Scene::new().await;
    facts(&s, s.rig.machine_id, reported()).await;
    let topic = s.tree.topic.base.id;
    let offered = options(&s, topic, Some(s.on.environment)).await;
    assert!(
        offered
            .providers
            .iter()
            .any(|p| p.provider.id == s.on.provider && p.usable)
    );
    assert!(
        offered
            .environments
            .iter()
            .any(|e| e.environment.id == s.on.environment && e.usable)
    );
    let project = options(&s, s.tree.project.base.id, None).await;
    assert!(
        project
            .environments
            .iter()
            .all(|e| !e.is_default && e.used != orch_api_contract::binding::EnvironmentUse::Topic)
    );
    let folder = s
        .rig
        .world
        .app
        .call(
            orch_api_contract::nodes::ProjectChildCreate,
            &[&s.tree.project.base.id],
        )
        .token(&s.ada.token)
        .body(&orch_api_contract::nodes::ProjectCreateBody {
            name: "Folder".to_owned(),
            description: None,
            access: None,
        })
        .send()
        .await
        .ok();
    let choices = options(&s, folder.base.id, None).await;
    assert!(
        choices
            .environments
            .iter()
            .all(|e| !e.is_default && e.used != orch_api_contract::binding::EnvironmentUse::Topic)
    );
    let threads: i64 = sqlx::query_scalar("SELECT count(*) FROM threads")
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(threads, 0, "reading choices creates no thread");
}

#[tokio::test]
async fn a_topics_manageable_default_is_judged_when_no_environment_is_named() {
    let s = Scene::new().await;
    facts(&s, s.rig.machine_id, reported()).await;
    sqlx::query("UPDATE agents SET machine_id = $2 WHERE id = $1")
        .bind(s.on.agent)
        .bind(s.rig.machine_id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO settings (node_id,key,value) VALUES ($1,'env.default_id',to_jsonb($2::uuid::text)) ON CONFLICT (node_id,key) DO UPDATE SET value=excluded.value")
        .bind(s.tree.topic.base.id)
        .bind(s.on.environment)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let choices = options(&s, s.tree.topic.base.id, None).await;
    let profile = choices
        .providers
        .iter()
        .find(|provider| provider.provider.id == s.on.provider)
        .unwrap()
        .agents
        .iter()
        .find(|agent| agent.agent.id == s.on.agent)
        .unwrap();
    assert!(profile.arrangements.iter().any(|arrangement| {
        arrangement.kind == ArrangementType::Together
            && arrangement.available
            && arrangement.permitted
    }));
    assert!(choices.environments.iter().any(|entry| {
        entry.environment.id == s.on.environment && entry.is_default && entry.usable
    }));
}

#[tokio::test]
async fn an_environment_named_before_creation_still_requires_the_callers_management() {
    let s = Scene::new().await;
    let private = binding_support::a_directory_on_a_machine(&s, "Private directory").await;
    let denied = s
        .rig
        .world
        .app
        .call(NodeBindingOptionsGet, &[&s.tree.topic.base.id])
        .token(&s.dan.token)
        .query(&NodeBindingOptionsGetQuery {
            environment_id: Some(private),
            ..Default::default()
        })
        .send()
        .await;
    assert_eq!(denied.problem().code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn a_profile_is_waited_for_and_reports_before_any_first_thread_or_turn() {
    let s = Scene::new().await;
    facts(&s, s.rig.machine_id, reported()).await;
    sqlx::query("UPDATE agents SET descriptor = '{}', machine_id = $2 WHERE id = $1")
        .bind(s.on.agent)
        .bind(s.rig.machine_id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let before = options(&s, s.tree.topic.base.id, Some(s.on.environment)).await;
    let profile = before
        .providers
        .iter()
        .find(|p| p.provider.id == s.on.provider)
        .unwrap()
        .agents
        .iter()
        .find(|a| a.agent.id == s.on.agent)
        .unwrap();
    assert!(!profile.usable);
    assert_eq!(profile.reason.as_deref(), Some("harness_unreported"));
    assert!(profile.arrangements.is_empty());
    s.rig
        .engine
        .discover_descriptors(s.rig.machine_id)
        .await
        .unwrap();
    let after = options(&s, s.tree.topic.base.id, Some(s.on.environment)).await;
    let profile = after
        .providers
        .iter()
        .find(|p| p.provider.id == s.on.provider)
        .unwrap()
        .agents
        .iter()
        .find(|a| a.agent.id == s.on.agent)
        .unwrap();
    assert!(
        profile.usable
            && profile
                .arrangements
                .iter()
                .any(|a| a.available && a.permitted)
    );
    assert_eq!(s.rig.commands.issued_of("turn.start"), 0);
    let threads: i64 = sqlx::query_scalar("SELECT count(*) FROM threads")
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(threads, 0);
}

#[tokio::test]
async fn enrollment_waits_for_facts_before_it_judges_the_machines_policy() {
    let s = Scene::new().await;
    facts(&s, s.rig.machine_id, json!({})).await;
    {
        let mut tx = s.rig.db.begin().await.unwrap();
        sqlx::query("UPDATE machines SET home_id=$2,org_id=$2 WHERE id=$1")
            .bind(s.rig.machine_id)
            .bind(s.ada.person.personal_org)
            .execute(&mut *tx)
            .await
            .unwrap();
        ownership::start_resource(
            &mut tx,
            ObjectRef {
                kind: ObjectKind::Machine,
                id: s.rig.machine_id,
            },
            s.ada.id(),
            NewAccess::OnlyMe,
            Time::new(s.rig.world.now()),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let body = MachineAgentCreateBody {
        home_id: s.tree.org.base.id,
        name: "Local harness".to_owned(),
        harness_kind: "fake".to_owned(),
        profile: None,
    };
    let waiting = s
        .rig
        .world
        .app
        .call(MachineAgentCreate, &[&s.rig.machine_id])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await;
    let waiting = waiting.problem();
    assert_eq!(waiting.code, ErrorCode::SourceOffline);
    assert_eq!(waiting.extra["reason"], "machine_facts_pending");
    facts(&s, s.rig.machine_id, reported()).await;
    s.rig
        .world
        .app
        .call(MachineAgentCreate, &[&s.rig.machine_id])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn a_provider_only_machine_offers_the_environment_harness_and_refuses_the_provider_harness_without_substitution()
 {
    let s = Scene::new().await;
    let mut descriptor = rig::descriptor();
    descriptor["arrangements"] = json!([
        {"type": "separated", "executor": "orch-exec", "executor_version": "*", "tool_sets": ["plain"]},
        {"type": "key_separated"}, {"type": "together"}
    ]);
    let reported_descriptor = descriptor.clone();
    s.rig.machine.handler.on(
        orch_link_contract::commands::CommandKind::AgentStart,
        move |_| {
            Ok(orch_link_contract::commands::CommandResult::AgentStart(
                orch_link_contract::commands::AgentStartResult {
                    reused: false,
                    harness_version: "1.0.0".to_owned(),
                    descriptor: serde_json::from_value(reported_descriptor.clone()).unwrap(),
                },
            ))
        },
    );
    sqlx::query("UPDATE agents SET descriptor = $2 WHERE id = $1")
        .bind(s.on.agent)
        .bind(descriptor)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    facts(&s, s.rig.machine_id, reported()).await;
    sqlx::query("UPDATE machines SET roles = ARRAY['provider'] WHERE id = $1")
        .bind(s.rig.machine_id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE providers SET kind = 'api', harness_kind = NULL, agent_id = NULL, protocols = ARRAY['openai-responses'], arrangements = ARRAY['separated','key_separated'], upstreams = $2 WHERE id = $1")
        .bind(s.on.provider).bind(json!([{"protocol":"openai-responses", "base_url":"https://model.example.test/v1", "options":{}}])).execute(s.rig.db.pool()).await.unwrap();
    let other = {
        let mut conn = s.rig.db.acquire().await.unwrap();
        orch_db::queries::resources::testing::insert_machine(
            &mut conn,
            "harness-machine",
            "connected",
        )
        .await
        .unwrap()
    };
    facts(&s, other, reported()).await;
    sqlx::query(
        "UPDATE machines SET roles = ARRAY['agent','environment'], online = true WHERE id = $1",
    )
    .bind(other)
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    let environment = binding_support::a_directory_on_a_machine(&s, "Harness workspace").await;
    sqlx::query("UPDATE environments SET machine_id = $2 WHERE id = $1")
        .bind(environment)
        .bind(other)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    binding_support::offering_plain(&s, environment).await;
    let offers = options(&s, s.tree.topic.base.id, Some(environment)).await;
    let provider = offers
        .providers
        .iter()
        .find(|p| p.provider.id == s.on.provider)
        .unwrap();
    let agent = provider
        .agents
        .iter()
        .find(|a| a.agent.id == s.on.agent)
        .unwrap();
    let first = agent.arrangements.iter().min_by_key(|a| a.rank).unwrap();
    assert_eq!(first.kind, ArrangementType::KeySeparated);
    assert!(first.available && first.permitted);
    assert_eq!(first.isolation.harness_runs_in, HarnessPlace::Environment);
    assert_eq!(
        first.isolation.held_by,
        orch_api_contract::binding::CredentialHolder::Provider
    );
    let separated = agent
        .arrangements
        .iter()
        .find(|a| a.kind == ArrangementType::Separated)
        .unwrap();
    assert!(!separated.available);
    assert!(separated.detail.as_deref().unwrap().contains("agent role"));
    let mut body = CreateThread {
        name: Some("First thread".to_owned()),
        binding: Some(CreateThreadBinding {
            agent_id: s.on.agent,
            provider_id: s.on.provider,
            arrangement: Some(ArrangementType::Separated),
        }),
        model: Some(rig::MODEL.to_owned()),
        environment: Some(EnvironmentChoice::Use(environment)),
        ..Default::default()
    };
    let refused = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await;
    let refused = refused.problem();
    assert_eq!(refused.code, ErrorCode::BindingIncompatible);
    assert_eq!(refused.extra["reason"], "agent_not_on_that_machine");
    body.binding.as_mut().unwrap().arrangement = None;
    let thread = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    let binding = thread.binding.as_ref().unwrap();
    assert_eq!(binding.arrangement, first.kind);
    assert_eq!(binding.isolation.harness_runs_in, HarnessPlace::Environment);
    let plan = s
        .rig
        .world
        .app
        .call(
            orch_api_contract::forks::ThreadForkPreflight,
            &[&thread.base.id],
        )
        .token(&s.ada.token)
        .body(&orch_api_contract::forks::ForkRequest {
            result: orch_api_contract::forks::ForkResult::New,
            topic_id: None,
            name: Some("Try again".to_owned()),
            access: None,
            history: None,
            to: None,
            environment: None,
            worktree: None,
            files_at: None,
            memory: None,
            directory: None,
            cwd: None,
            options: None,
            check_fingerprints: None,
            fingerprint_request_id: None,
            mounts: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(plan.to.isolation.harness_runs_in, HarnessPlace::Environment);
    assert_eq!(
        plan.to.isolation.held_by,
        orch_api_contract::binding::CredentialHolder::Provider
    );
    s.rig.engine.discover_descriptors(other).await.unwrap();
    let mut unsupported = reported();
    unsupported["policy"]["agent.harnesses"] = json!([]);
    facts(&s, other, unsupported).await;
    let refused = options(&s, s.tree.topic.base.id, Some(environment)).await;
    let agent = refused
        .providers
        .iter()
        .find(|provider| provider.provider.id == s.on.provider)
        .unwrap()
        .agents
        .iter()
        .find(|agent| agent.agent.id == s.on.agent)
        .unwrap();
    let beside_tools = agent
        .arrangements
        .iter()
        .find(|arrangement| arrangement.kind == ArrangementType::KeySeparated)
        .unwrap();
    assert!(!beside_tools.available);
    assert!(beside_tools.detail.as_deref().unwrap().contains("allowed"));
    facts(&s, other, reported()).await;
    sqlx::query("UPDATE machines SET roles = ARRAY['agent'] WHERE id = $1")
        .bind(s.rig.machine_id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let refused = options(&s, s.tree.topic.base.id, Some(environment)).await;
    let profile = refused
        .providers
        .iter()
        .find(|provider| provider.provider.id == s.on.provider)
        .unwrap()
        .agents
        .iter()
        .find(|agent| agent.agent.id == s.on.agent)
        .unwrap();
    let forwarded = profile
        .arrangements
        .iter()
        .find(|arrangement| arrangement.kind == ArrangementType::KeySeparated)
        .unwrap();
    assert!(!forwarded.available);
    assert!(
        forwarded
            .detail
            .as_deref()
            .unwrap()
            .contains("provider role")
    );
}

#[tokio::test]
async fn a_harness_the_machine_cannot_run_says_why_once_as_its_state_and_is_asked_again_only_after_a_pause()
 {
    use orch_link_contract::commands::{CommandError, CommandKind, ErrorCode as LinkCode};
    let s = Scene::new().await;
    facts(&s, s.rig.machine_id, reported()).await;
    sqlx::query("UPDATE agents SET descriptor = '{}', machine_id = $2 WHERE id = $1")
        .bind(s.on.agent)
        .bind(s.rig.machine_id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    s.rig.seen.refuse(
        CommandKind::AgentStart,
        CommandError::new(
            LinkCode::Unsupported,
            "agent.harnesses: this machine lists no program for fake",
        ),
    );
    s.rig
        .engine
        .discover_descriptors(s.rig.machine_id)
        .await
        .unwrap();
    let asked = s.rig.seen.of_kind(CommandKind::AgentStart).len();
    assert!(asked >= 1);
    let state: (String, Option<String>) = sqlx::query_as(
        "SELECT status, status_detail FROM harnesses WHERE agent_id = $1 AND machine_id = $2",
    )
    .bind(s.on.agent)
    .bind(s.rig.machine_id)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(state.0, "error", "the reason is the harness's state");
    assert_eq!(
        state.1.as_deref(),
        Some("agent.harnesses: this machine lists no program for fake")
    );
    // The machine reports itself every few seconds; it is not asked again at each report.
    s.rig
        .engine
        .discover_descriptors(s.rig.machine_id)
        .await
        .unwrap();
    s.rig
        .engine
        .discover_descriptors(s.rig.machine_id)
        .await
        .unwrap();
    assert_eq!(
        s.rig.seen.of_kind(CommandKind::AgentStart).len(),
        asked,
        "not asked again within the pause"
    );
    s.rig
        .world
        .advance(orch_engine::binding::DISCOVERY_RETRY_AFTER + chrono::Duration::minutes(1));
    s.rig
        .engine
        .discover_descriptors(s.rig.machine_id)
        .await
        .unwrap();
    assert!(
        s.rig.seen.of_kind(CommandKind::AgentStart).len() > asked,
        "asked again after the pause"
    );
    let status: String =
        sqlx::query_scalar("SELECT status FROM harnesses WHERE agent_id = $1 AND machine_id = $2")
            .bind(s.on.agent)
            .bind(s.rig.machine_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        status, "online",
        "an installation repaired meanwhile is found"
    );
}

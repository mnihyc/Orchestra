//! What a person's machine has is registered from it by its owner and by nobody else: a profile for a harness it drives, an API whose key is typed on it, a login its harness already holds; the first owner stays the machine's owner, the provider is used there and nowhere else, and what the machine did not report cannot be registered.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{
    Agent, MachineAgentCreate, MachineAgentCreateBody, MachineProviderCreate,
    MachineProviderCreateBody, Provider, ProviderGet, ProviderKeySet, ProviderKeySetBody,
    ProviderList, ProviderListQuery, ProviderStatus, Upstream,
};
use orch_link_contract::commands::{CommandKind, CommandResult, ProviderRefreshResult};
use orch_types::Id;
use places::{Who, make_department};
use rig::Scene;
use serde_json::{Value, json};

const URL: &str = "https://api.example.com/v1";

/// What a connector reports of a machine that drives the scripted harness and may forward model calls
/// to `URL`.
fn reported() -> Value {
    json!({
        // The directory runtime service a connector declares, which a provider's runtime there uses.
        "provider_runtime_descriptor": { "lifecycle": "attach" },
        "adapters": [{ "kind": "fake", "adapter_version": 1, "builtin": true }],
        "harnesses": [{"kind": "fake", "path": "/test/harness", "version": "1.0.0", "daemon": false}],
        "policy": { "provider.upstreams": [URL], "provider.arrangements": ["separated", "key_separated"],
            "agent.harnesses": [{"kind": "fake", "path": "/test/harness"}] }
    })
}

/// Makes Ada the owner of the scene's machine, as enrolling it would have, and has the machine
/// report `facts`: it is at home in her own org, and she is its Owners row, which is what owning it is.
async fn ada_owns_the_machine(s: &Scene, facts: Value) {
    sqlx::query(
        "UPDATE machines SET owner_id = $2, facts = $3, home_id = n.id, org_id = n.org_id
           FROM users u, nodes n WHERE machines.id = $1 AND u.id = $2 AND n.id = u.personal_org_id",
    )
    .bind(s.rig.machine_id)
    .bind(s.ada.id())
    .bind(facts)
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO permission_entries (id, object_kind, object_id, page, permission, holder_kind, holder_id,
                                         created_by, source)
         VALUES ($1, 'machine', $2, 'live', 'access.own', 'user', $3, $3, 'creation')
         ON CONFLICT DO NOTHING",
    )
    .bind(orch_types::new_id())
    .bind(s.rig.machine_id)
    .bind(s.ada.id())
    .execute(s.rig.db.pool())
    .await
    .unwrap();
}

/// The machine answers what the engine asks of a login: it is signed in, with this plan.
fn the_login_is_there(s: &Scene, status: &'static str) {
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderRefresh, move |_| {
            Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {
                source: None,
                status: status.to_owned(),
                facts: serde_json::from_value(if status == "needs_setup" {
                    json!({})
                } else {
                    json!({ "plan": "pro", "windows": [] })
                })
                .unwrap(),
            }))
        });
}

/// Whether a refusal says that the member `path` of the request is what is wrong.
fn names(problem: &orch_api_contract::common::Problem, path: &str) -> bool {
    problem.extra["errors"]
        .as_array()
        .is_some_and(|faults| faults.iter().any(|fault| fault["path"] == path))
}

fn upstream() -> Upstream {
    Upstream {
        protocol: "openai-responses".to_owned(),
        base_url: URL.to_owned(),
        options: json!({}),
    }
}

async fn register_agent(
    s: &Scene,
    by: &Who,
    home: Id,
    kind: &str,
) -> orch_api_core::testing::Reply<Agent> {
    s.rig
        .world
        .app
        .call(MachineAgentCreate, &[&s.rig.machine_id])
        .token(&by.token)
        .body(&MachineAgentCreateBody {
            home_id: home,
            harness_kind: kind.to_owned(),
            name: format!("{kind} here"),
            profile: None,
        })
        .send()
        .await
}

async fn register_login(
    s: &Scene,
    by: &Who,
    home: Id,
    agent: Id,
) -> orch_api_core::testing::Reply<Provider> {
    s.rig
        .world
        .app
        .call(MachineProviderCreate, &[&s.rig.machine_id])
        .token(&by.token)
        .body(&MachineProviderCreateBody::Subscription {
            home_id: home,
            name: "Ada's login".to_owned(),
            agent_id: agent,
            sources: None,
        })
        .send()
        .await
}

async fn register_api(s: &Scene, by: &Who, home: Id) -> orch_api_core::testing::Reply<Provider> {
    s.rig
        .world
        .app
        .call(MachineProviderCreate, &[&s.rig.machine_id])
        .token(&by.token)
        .body(&MachineProviderCreateBody::Api {
            home_id: home,
            name: "Ada's key".to_owned(),
            upstreams: vec![upstream()],
            sources: None,
        })
        .send()
        .await
}

// ---------------------------------------------------------------- a profile for a harness the machine drives

#[tokio::test]
async fn the_owner_of_a_machine_registers_a_profile_for_a_harness_it_reported_and_it_is_theirs() {
    let s = Scene::new().await;
    ada_owns_the_machine(&s, reported()).await;
    let org = s.tree.org.base.id;
    let made = register_agent(&s, &s.ada, org, "fake").await.ok();
    assert_eq!(made.base.harness_kind, "fake");
    assert_eq!(
        made.base.machine.as_ref().map(|m| m.id),
        Some(s.rig.machine_id)
    );
    assert_eq!(
        made.owner.as_ref().map(|o| o.username.as_str()),
        Some("ada")
    );
    assert_eq!(made.home.as_ref().map(|h| h.id), Some(org));
    assert_eq!(
        made.descriptor,
        json!({}),
        "no harness ran, and nothing says yet what it is"
    );
}

#[tokio::test]
async fn what_the_machine_did_not_report_it_can_drive_cannot_be_registered_on_it() {
    let s = Scene::new().await;
    ada_owns_the_machine(&s, reported()).await;
    let org = s.tree.org.base.id;
    let refused = register_agent(&s, &s.ada, org, "codex").await.problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert!(names(&refused, "harness_kind"), "{refused:?}");
    // A machine that has not reported anything has not said that it drives any.
    ada_owns_the_machine(&s, json!({})).await;
    let silent = register_agent(&s, &s.ada, org, "fake").await.problem();
    assert_eq!(silent.code, ErrorCode::SourceOffline);
    assert_eq!(silent.extra["reason"], "machine_facts_pending");
}

#[tokio::test]
async fn only_the_owner_of_a_machine_registers_on_it_and_only_in_their_own_domain() {
    let s = Scene::new().await;
    ada_owns_the_machine(&s, reported()).await;
    let org = s.tree.org.base.id;
    let stranger = register_agent(&s, &s.bea, org, "fake").await.problem();
    assert_eq!(
        stranger.code,
        ErrorCode::NotFound,
        "a machine of somebody else's is not there"
    );
    let developer = register_agent(&s, &s.eve, org, "fake").await.problem();
    assert_eq!(
        developer.code,
        ErrorCode::NotFound,
        "a member of the org does not own the machine"
    );
    let nobody = s
        .rig
        .world
        .app
        .call(MachineAgentCreate, &[&s.rig.machine_id])
        .body(&MachineAgentCreateBody {
            home_id: org,
            harness_kind: "fake".to_owned(),
            name: "x".to_owned(),
            profile: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
    // Ada's domain is Acme; an org of Bea's is not inside it.
    let theirs = places::make_org(&s.rig.world, &s.bea, "Other", "other").await;
    let outside = register_agent(&s, &s.ada, theirs.base.id, "fake")
        .await
        .problem();
    assert_eq!(outside.code, ErrorCode::RequestInvalid);
    assert!(names(&outside, "home_id"), "{outside:?}");
}

// ---------------------------------------------------------------- an API whose key is typed on the machine

#[tokio::test]
async fn an_api_registered_from_a_machine_is_run_there_and_its_key_is_kept_there() {
    let s = Scene::new().await;
    ada_owns_the_machine(&s, reported()).await;
    let org = s.tree.org.base.id;
    let made = register_api(&s, &s.ada, org).await.ok();
    assert_eq!(
        made.base.placement,
        orch_api_contract::environment::Placement::Connected
    );
    assert_eq!(made.machine.as_ref().map(|m| m.id), Some(s.rig.machine_id));
    assert_eq!(
        made.key_machine.as_ref().map(|m| m.id),
        Some(s.rig.machine_id)
    );
    assert!(!made.key_set, "the server holds no key of it");
    assert_eq!(made.owner.username, "ada");
    assert_eq!(
        made.base.status,
        ProviderStatus::NeedsSetup,
        "it has no model yet"
    );
    let allowed: Vec<_> = made
        .arrangements
        .iter()
        .filter(|each| each.allowed)
        .map(|each| each.kind)
        .collect();
    use orch_api_contract::binding::ArrangementType::{KeySeparated, Separated};
    assert_eq!(
        allowed,
        [Separated, KeySeparated],
        "the key stays where it is kept"
    );
}

#[tokio::test]
async fn an_api_is_registered_only_where_the_machine_may_forward_to_and_only_if_it_plays_the_role()
{
    let s = Scene::new().await;
    let org = s.tree.org.base.id;
    ada_owns_the_machine(
        &s,
        json!({ "adapters": [], "policy": { "provider.upstreams": ["https://elsewhere.example/v1"] } }),
    )
    .await;
    let refused = register_api(&s, &s.ada, org).await.problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert!(
        names(&refused, "upstreams[0].base_url"),
        "the machine's local policy does not let it forward there: {refused:?}"
    );
    ada_owns_the_machine(&s, reported()).await;
    sqlx::query("UPDATE machines SET roles = ARRAY['agent','environment'] WHERE id = $1")
        .bind(s.rig.machine_id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let without_role = register_api(&s, &s.ada, org).await.problem();
    assert_eq!(without_role.code, ErrorCode::RequestInvalid);
    let stranger = register_api(&s, &s.bea, org).await.problem();
    assert_eq!(stranger.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn replacing_a_machine_registered_key_returns_the_provider_without_moving_either_location() {
    let s = Scene::new().await;
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([7; 32]),
    );
    ada_owns_the_machine(&s, reported()).await;
    let org = s.tree.org.base.id;
    let made = register_api(&s, &s.ada, org).await.ok();
    let id = made.base.id;
    let given = s
        .rig
        .world
        .app
        .call(ProviderKeySet, &[&id])
        .token(&s.ada.token)
        .body(&ProviderKeySetBody {
            api_key: "sk-held-by-the-server-0001".to_owned(),
        })
        .send()
        .await;
    assert_eq!(given.status, 200, "{:?}", given.body);
    let saved = given.ok();
    assert!(saved.key_set);
    assert_eq!(
        saved.credential_keeper.as_ref().map(|machine| machine.id),
        Some(s.rig.machine_id)
    );
    let read = s
        .rig
        .world
        .app
        .call(ProviderGet, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert!(read.key_set, "the replacement key was saved");
    assert!(
        read.key_machine.is_none(),
        "the saved key needs no manual key-entry prompt on that machine"
    );
    assert_eq!(
        read.machine.as_ref().map(|m| m.id),
        Some(s.rig.machine_id),
        "the machine is still where the provider runs"
    );
    assert_eq!(
        read.base.placement,
        orch_api_contract::environment::Placement::Connected
    );
}

// ---------------------------------------------------------------- a login the harness already has there

#[tokio::test]
async fn a_login_found_signed_in_on_the_machine_is_a_provider_that_is_used_there_and_nowhere_else()
{
    let s = Scene::new().await;
    ada_owns_the_machine(&s, reported()).await;
    let org = s.tree.org.base.id;
    let agent = register_agent(&s, &s.ada, org, "fake").await.ok();
    the_login_is_there(&s, "ready");
    let made = register_login(&s, &s.ada, org, agent.base.id).await.ok();
    assert_eq!(made.base.status, ProviderStatus::NeedsSetup);
    assert!(
        made.models.is_empty(),
        "the sign-in is present before any model is enabled"
    );
    assert_eq!(made.base.harness_kind.as_deref(), Some("fake"));
    assert_eq!(
        made.base.placement,
        orch_api_contract::environment::Placement::Connected
    );
    assert_eq!(made.machine.as_ref().map(|m| m.id), Some(s.rig.machine_id));
    assert_eq!(made.agent.as_ref().map(|a| a.id), Some(agent.base.id));
    assert!(
        made.facts.plan.is_none(),
        "registration does not probe the harness"
    );
    let allowed: Vec<_> = made
        .arrangements
        .iter()
        .filter(|each| each.allowed)
        .map(|each| each.kind)
        .collect();
    assert_eq!(
        allowed,
        [orch_api_contract::binding::ArrangementType::Separated],
        "no copy of the login is allowed to go anywhere"
    );
    assert!(made.harnesses.is_empty(), "registration starts no harness");
    let declared: Value = sqlx::query_scalar("SELECT descriptor FROM agents WHERE id = $1")
        .bind(agent.base.id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(declared, json!({}));
    assert_eq!(s.rig.commands.issued_of("provider.refresh"), 0);
}

#[tokio::test]
async fn the_first_owner_of_a_registered_login_is_the_machines_owner_and_stays_so() {
    let s = Scene::new().await;
    ada_owns_the_machine(&s, reported()).await;
    let org = s.tree.org.base.id;
    let agent = register_agent(&s, &s.ada, org, "fake").await.ok();
    the_login_is_there(&s, "ready");
    let made = register_login(&s, &s.ada, org, agent.base.id).await.ok();
    assert_eq!(made.owner.username, "ada");
    // Another person is named an owner of it; the first owner is the same one.
    s.rig
        .lets_use("provider", made.base.id, "access.own", &s.eve, json!({}))
        .await;
    let read = s
        .rig
        .world
        .app
        .call(ProviderGet, &[&made.base.id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(read.owner.username, "ada");
    assert_eq!(read.owners, 2);
}

#[tokio::test]
async fn a_registered_login_waits_for_its_first_observation_without_probing_the_harness() {
    let s = Scene::new().await;
    ada_owns_the_machine(&s, reported()).await;
    let org = s.tree.org.base.id;
    let agent = register_agent(&s, &s.ada, org, "fake").await.ok();
    the_login_is_there(&s, "needs_setup");
    let made = register_login(&s, &s.ada, org, agent.base.id).await.ok();
    assert_eq!(made.base.status, ProviderStatus::NeedsSetup);
    assert!(made.facts.plan.is_none());
    assert_eq!(s.rig.commands.issued_of("provider.refresh"), 0);
    let listed = s
        .rig
        .world
        .app
        .call(ProviderList, &[])
        .token(&s.ada.token)
        .query(&ProviderListQuery {
            org_id: None,
            scope: None,
            ..ProviderListQuery::default()
        })
        .send()
        .await
        .ok();
    assert!(
        listed
            .items
            .into_iter()
            .filter_map(orch_api_contract::common::Listed::into_visible)
            .any(|each| each.base.id == made.base.id),
        "the newly registered provider is listed while it awaits observation"
    );
}

#[tokio::test]
async fn a_machine_that_is_away_keeps_its_registration_without_an_online_probe() {
    let s = Scene::new().await;
    ada_owns_the_machine(&s, reported()).await;
    let org = s.tree.org.base.id;
    let agent = register_agent(&s, &s.ada, org, "fake").await.ok();
    sqlx::query("UPDATE machines SET online = false WHERE id = $1")
        .bind(s.rig.machine_id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let made = register_login(&s, &s.ada, org, agent.base.id).await.ok();
    assert_eq!(
        made.machine.as_ref().map(|machine| machine.id),
        Some(s.rig.machine_id)
    );
    assert_eq!(made.base.status, ProviderStatus::NeedsSetup);
    assert_eq!(s.rig.commands.issued_of("provider.refresh"), 0);
}

#[tokio::test]
async fn a_harness_the_org_does_not_allow_as_a_subscription_is_refused_and_the_refusal_names_the_decision()
 {
    let s = Scene::new().await;
    ada_owns_the_machine(&s, reported()).await;
    let org = s.tree.org.base.id;
    let agent = register_agent(&s, &s.ada, org, "fake").await.ok();
    the_login_is_there(&s, "ready");
    s.rig
        .world
        .set_at(
            orch_settings::Target::Node(org),
            "providers.subscription_harnesses",
            json!(["codex"]),
        )
        .await;
    let refused = register_login(&s, &s.ada, org, agent.base.id)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ProviderSubscriptionNotAllowed);
    assert_eq!(refused.extra["harness_kind"], "fake");
}

#[tokio::test]
async fn each_explicit_login_registration_creates_a_provider_in_the_home_it_names() {
    let s = Scene::new().await;
    ada_owns_the_machine(&s, reported()).await;
    let org = s.tree.org.base.id;
    let agent = register_agent(&s, &s.ada, org, "fake").await.ok();
    the_login_is_there(&s, "ready");
    let first = register_login(&s, &s.ada, org, agent.base.id).await.ok();
    let again = register_login(&s, &s.ada, org, agent.base.id).await.ok();
    assert_ne!(again.base.id, first.base.id);
    assert_eq!(again.home.id, org);
    let inside = make_department(&s.rig.world, &s.ada, org, "Research", "research").await;
    let elsewhere = register_login(&s, &s.ada, inside.base.id, agent.base.id)
        .await
        .ok();
    assert_ne!(elsewhere.base.id, first.base.id);
    assert_eq!(elsewhere.home.id, inside.base.id);
}

#[tokio::test]
async fn only_the_owner_of_the_machine_registers_its_login() {
    let s = Scene::new().await;
    ada_owns_the_machine(&s, reported()).await;
    let org = s.tree.org.base.id;
    let agent = register_agent(&s, &s.ada, org, "fake").await.ok();
    the_login_is_there(&s, "ready");
    let stranger = register_login(&s, &s.bea, org, agent.base.id)
        .await
        .problem();
    assert_eq!(stranger.code, ErrorCode::AuthzDenied);
    let nobody = s
        .rig
        .world
        .app
        .call(MachineProviderCreate, &[&s.rig.machine_id])
        .body(&MachineProviderCreateBody::Subscription {
            home_id: org,
            name: "x".to_owned(),
            agent_id: agent.base.id,
            sources: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
    // A profile that is no profile of this machine cannot carry a login registered from it.
    let foreign = register_login(&s, &s.ada, org, orch_types::new_id())
        .await
        .problem();
    assert_eq!(foreign.code, ErrorCode::RequestInvalid);
    assert!(names(&foreign, "agent_id"), "{foreign:?}");
}

// ---------------------------------------------------------------- a turn on what was registered

#[tokio::test]
async fn a_provider_on_a_key_the_server_holds_runs_on_the_machine_it_was_registered_from_and_serves_a_turn()
 {
    use orch_api_contract::binding::EnvironmentChoice;
    use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, ThreadCreate};
    use orch_api_contract::resources::{ProviderModelCreate, ProviderModelCreateBody};
    use orch_link_contract::commands::Secret;
    use std::sync::{Arc, Mutex};
    let s = Scene::new().await;
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([7; 32]),
    );
    ada_owns_the_machine(&s, reported()).await;
    let started = Arc::new(Mutex::new(Vec::new()));
    {
        let started = Arc::clone(&started);
        s.rig
            .machine
            .handler
            .on(CommandKind::ProviderStart, move |command| {
                if let orch_link_contract::commands::Command::ProviderStart(start) = command {
                    started.lock().unwrap().push(start.clone());
                }
                Ok(CommandResult::ProviderStart(
                    orch_link_contract::commands::Empty {},
                ))
            });
    }
    let org = s.tree.org.base.id;
    let made = register_api(&s, &s.ada, org).await.ok();
    let key = "sk-held-by-the-server-0002";
    let given = s
        .rig
        .world
        .app
        .call(ProviderKeySet, &[&made.base.id])
        .token(&s.ada.token)
        .body(&ProviderKeySetBody {
            api_key: key.to_owned(),
        })
        .send()
        .await;
    assert_eq!(given.status, 200, "{:?}", given.body);
    s.rig
        .world
        .app
        .call(ProviderModelCreate, &[&made.base.id])
        .token(&s.ada.token)
        .body(&ProviderModelCreateBody {
            model_name: rig::MODEL.to_owned(),
            display_name: None,
            parameters: None,
            groups: None,
        })
        .send()
        .await
        .ok();
    let ready = s
        .rig
        .world
        .app
        .call(ProviderGet, &[&made.base.id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(ready.base.status, ProviderStatus::Ready);
    let thread = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&CreateThread {
            name: Some("On the machine".to_owned()),
            binding: Some(CreateThreadBinding {
                agent_id: s.on.agent,
                provider_id: made.base.id,
                arrangement: None,
            }),
            model: Some(rig::MODEL.to_owned()),
            environment: Some(EnvironmentChoice::Use(s.on.environment)),
            directory: Some("web".to_owned()),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok();
    s.exchange(thread.base.id, 0, "Hello", "Hi").await;
    let starts = started.lock().unwrap().clone();
    assert_eq!(
        starts.len(),
        1,
        "the provider was started on the machine, once"
    );
    assert_eq!(starts[0].provider_id, made.base.id);
    assert!(
        matches!(starts[0].credential, Some(Secret::Ref(_))),
        "the key goes as a reference to its secret, sealed to the machine when the command is written: {:?}",
        starts[0].credential
    );
    assert_eq!(
        s.rig.seen.of_kind(CommandKind::LeaseOpen).len(),
        1,
        "the lease of the turn is opened on the machine that runs the provider"
    );
}

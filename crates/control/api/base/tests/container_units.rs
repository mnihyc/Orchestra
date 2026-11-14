//! A unit that a host runs (the container of a provider or of an environment) is no resource of its own: the host that runs it names it, so every read that mentions it answers as it does for that host and none fails whole for it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

#[path = "container_units/fixture.rs"]
mod fixture;
mod places;
mod rig;
mod support;

use orch_api_contract::binding::{NodeBindingOptionsGet, NodeBindingOptionsGetQuery};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::gateways::MachineGatewayGet;
use orch_api_contract::resources::{
    MachinePolicyGet, MachineProviderCreate, MachineProviderCreateBody, Upstream,
};
use orch_api_contract::threads::{ThreadOpeningGet, ThreadOpeningGetQuery};
use orch_link_contract::policy::{LocalPolicy, PolicyDefaults};
use orch_link_contract::session::Role;
use orch_types::Id;
use rig::Scene;

/// The scene's host, which Ada made and holds alone, and the template its runtimes are made from.
async fn host_of_the_scene(s: &Scene) -> (Id, Id) {
    let mut conn = s.rig.db.acquire().await.unwrap();
    fixture::host_and_template(
        &mut conn,
        s.ada.id(),
        s.tree.org.base.id,
        "Unit",
        s.rig.world.now(),
    )
    .await
}

/// The scene's provider, kept in a new container of Ada's host: its host and the unit's machine.
async fn the_provider_in_a_container(s: &Scene) -> (Id, Id) {
    let (host, template) = host_of_the_scene(s).await;
    let mut conn = s.rig.db.acquire().await.unwrap();
    let unit =
        fixture::provider_unit(&mut conn, host, template, s.on.provider, s.rig.world.now()).await;
    (host, unit)
}

/// The scene's environment, running in a new container of Ada's host: its host and the unit's machine.
async fn the_environment_in_a_container(s: &Scene) -> (Id, Id) {
    let (host, template) = host_of_the_scene(s).await;
    let mut conn = s.rig.db.acquire().await.unwrap();
    let unit = fixture::environment_unit(
        &mut conn,
        host,
        template,
        s.on.environment,
        s.rig.world.now(),
    )
    .await;
    (host, unit)
}

/// What the host's node reported of its policy, with `handed` handed over to be changed from the
/// system. A unit reports no policy of its own (the system made its connector and holds it to
/// nothing), so the policy page of a unit shows the policy of the node of the host that runs it,
/// as that node reports it: the roles it plays, which is the node alone.
async fn the_host_reports_a_policy(s: &Scene, host: Id, handed: &[&str]) {
    let mut policy = LocalPolicy::installed(PolicyDefaults::Managed);
    policy.roles = Some(vec![Role::Node]);
    policy.policy_delegated = handed.iter().map(|key| (*key).to_owned()).collect();
    sqlx::query(
        "UPDATE machines m SET facts = jsonb_set(m.facts, '{policy}', $2)
           FROM hosts h WHERE h.id = $1 AND m.id = h.machine_id",
    )
    .bind(host)
    .bind(policy.reported())
    .execute(s.rig.db.pool())
    .await
    .unwrap();
}

async fn options_of_the_topic(s: &Scene) -> orch_api_contract::binding::BindingOptions {
    s.rig
        .world
        .app
        .call(NodeBindingOptionsGet, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .query(&NodeBindingOptionsGetQuery::default())
        .send()
        .await
        .ok()
}

async fn opening_of(s: &Scene, thread: Id) -> orch_api_contract::threads::ThreadOpening {
    s.rig
        .world
        .app
        .call(ThreadOpeningGet, &[&thread])
        .token(&s.ada.token)
        .query(&ThreadOpeningGetQuery::default())
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn the_binding_options_of_a_topic_offer_a_provider_whose_connection_runs_in_a_container() {
    let s = Scene::new().await;
    the_provider_in_a_container(&s).await;
    let options = options_of_the_topic(&s).await;
    assert!(
        options
            .providers
            .iter()
            .any(|each| each.provider.id == s.on.provider),
        "the provider is offered"
    );
}

#[tokio::test]
async fn the_binding_options_of_a_topic_are_read_whose_environment_runs_in_a_container() {
    let s = Scene::new().await;
    the_environment_in_a_container(&s).await;
    let options = options_of_the_topic(&s).await;
    assert!(
        options
            .providers
            .iter()
            .any(|each| each.provider.id == s.on.provider),
        "the provider is offered beside an environment that runs in a container"
    );
    assert!(
        options
            .environments
            .iter()
            .any(|each| each.environment.id == s.on.environment),
        "the environment is offered"
    );
}

#[tokio::test]
async fn a_thread_on_a_provider_whose_connection_runs_in_a_container_opens() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    the_provider_in_a_container(&s).await;
    let opening = opening_of(&s, thread).await;
    assert_eq!(opening.sync.thread.base.id, thread);
}

#[tokio::test]
async fn a_thread_in_an_environment_that_runs_in_a_container_opens() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    the_environment_in_a_container(&s).await;
    let opening = opening_of(&s, thread).await;
    assert_eq!(opening.sync.thread.base.id, thread);
}

async fn the_policy_of(s: &Scene, unit: Id) -> orch_api_contract::resources::MachinePolicy {
    s.rig
        .world
        .app
        .call(MachinePolicyGet, &[&unit])
        .token(&s.ada.token)
        .send()
        .await
        .ok()
}

async fn the_policy_is_not_there_for_a_stranger(s: &Scene, unit: Id) {
    let refused = s
        .rig
        .world
        .app
        .call(MachinePolicyGet, &[&unit])
        .token(&s.bea.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn the_policy_of_a_provider_unit_is_read_by_whoever_manages_its_host_and_by_nobody_else() {
    let s = Scene::new().await;
    let (host, unit) = the_provider_in_a_container(&s).await;
    the_host_reports_a_policy(&s, host, &["update.auto"]).await;
    let policy = the_policy_of(&s, unit).await;
    assert_eq!(policy.machine.id, unit);
    let key = |name: &str| {
        policy
            .keys
            .iter()
            .find(|key| key.key == name)
            .expect("the key is shown")
    };
    let handed = key("update.auto");
    assert!(
        handed.delegated && handed.writable,
        "the manager of the host may change what the host's node handed over"
    );
    let kept = key("agent.max_sandbox");
    assert!(
        !kept.delegated && !kept.writable,
        "what the host's node kept stays kept"
    );
    the_policy_is_not_there_for_a_stranger(&s, unit).await;
}

#[tokio::test]
async fn the_policy_of_an_environment_unit_is_read_by_whoever_manages_its_host_and_by_nobody_else()
{
    let s = Scene::new().await;
    let (host, unit) = the_environment_in_a_container(&s).await;
    the_host_reports_a_policy(&s, host, &["mcp.add"]).await;
    let policy = the_policy_of(&s, unit).await;
    assert_eq!(policy.machine.id, unit);
    let key = |name: &str| {
        policy
            .keys
            .iter()
            .find(|key| key.key == name)
            .expect("the key is shown")
    };
    let handed = key("mcp.add");
    assert!(
        handed.delegated && handed.writable,
        "the manager of the host may change what the host's node handed over"
    );
    let kept = key("environment.terminal");
    assert!(
        !kept.delegated && !kept.writable,
        "what the host's node kept stays kept"
    );
    the_policy_is_not_there_for_a_stranger(&s, unit).await;
}

async fn a_shared_hosts_user_cannot_register_on_another_persons_unit(
    s: &Scene,
    host: Id,
    unit: Id,
) {
    use orch_access_contract::decisions::ObjectRef;
    use orch_access_contract::permissions::{ObjectKind, Permission};
    use orch_authz::{Caller, Policy, Principal};
    s.rig
        .lets_use("host", host, "host.use", &s.bea, serde_json::json!({}))
        .await;
    let home = places::make_org(&s.rig.world, &s.bea, "Other home", "other-home").await;
    let mut conn = s.rig.db.acquire().await.unwrap();
    let principal = Principal::load(
        &mut conn,
        Caller::session(s.bea.id()),
        orch_types::Time::new(s.rig.world.now()),
    )
    .await
    .unwrap();
    assert!(
        orch_authz::resources::check(
            &mut conn,
            &principal,
            ObjectRef {
                kind: ObjectKind::Host,
                id: host
            },
            Permission::HostUse,
            Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    assert!(
        !orch_authz::resources::check_machine(
            &mut conn,
            &principal,
            unit,
            Permission::MachineUse,
            Policy::default()
        )
        .await
        .unwrap()
        .is_allowed(),
        "using a shared host does not grant changes inside somebody else's running work"
    );
    let manager = Principal::load(
        &mut conn,
        Caller::session(s.ada.id()),
        orch_types::Time::new(s.rig.world.now()),
    )
    .await
    .unwrap();
    assert!(
        orch_authz::resources::check_machine(
            &mut conn,
            &manager,
            unit,
            Permission::MachineUse,
            Policy::default()
        )
        .await
        .unwrap()
        .is_allowed(),
        "the host manager can still administer its units"
    );
    drop(conn);
    let refused = s
        .rig
        .world
        .app
        .call(MachineProviderCreate, &[&unit])
        .token(&s.bea.token)
        .body(&MachineProviderCreateBody::Api {
            home_id: Some(home.base.id),
            name: "Other person's credential keeper".into(),
            upstreams: vec![Upstream {
                protocol: "openai-responses".into(),
                base_url: "https://api.example.com/v1".into(),
                options: serde_json::json!({}),
            }],
            sources: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn using_a_shared_host_does_not_register_a_provider_in_another_persons_provider_unit() {
    let s = Scene::new().await;
    let (host, unit) = the_provider_in_a_container(&s).await;
    a_shared_hosts_user_cannot_register_on_another_persons_unit(&s, host, unit).await;
}

#[tokio::test]
async fn using_a_shared_host_does_not_register_a_provider_in_another_persons_environment_unit() {
    let s = Scene::new().await;
    let (host, unit) = the_environment_in_a_container(&s).await;
    a_shared_hosts_user_cannot_register_on_another_persons_unit(&s, host, unit).await;
}

async fn the_gateway_status_of(
    s: &Scene,
    unit: Id,
) -> orch_api_contract::gateways::ConnectorGatewayStatus {
    s.rig
        .world
        .app
        .call(MachineGatewayGet, &[&unit])
        .token(&s.ada.token)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn the_gateway_status_of_a_provider_unit_is_read_by_whoever_sees_its_host() {
    let s = Scene::new().await;
    let (_, unit) = the_provider_in_a_container(&s).await;
    let status = the_gateway_status_of(&s, unit).await;
    assert_eq!(status.machine_id, unit);
    assert!(!status.connected);
    let refused = s
        .rig
        .world
        .app
        .call(MachineGatewayGet, &[&unit])
        .token(&s.bea.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn the_gateway_status_of_an_environment_unit_is_read_by_whoever_sees_its_host() {
    let s = Scene::new().await;
    let (_, unit) = the_environment_in_a_container(&s).await;
    let status = the_gateway_status_of(&s, unit).await;
    assert_eq!(status.machine_id, unit);
    assert!(!status.connected);
}

#[tokio::test]
async fn the_gateway_status_of_the_node_of_a_host_is_read_by_whoever_sees_the_host() {
    let s = Scene::new().await;
    let (host, _) = the_provider_in_a_container(&s).await;
    let node: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id = $1")
        .bind(host)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    let status = the_gateway_status_of(&s, node).await;
    assert_eq!(status.machine_id, node);
}

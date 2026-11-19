//! Agent collections bound their actual Provider/model and physical-machine sources.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod resources;
mod support;

use http::StatusCode;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyScope;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::directory::{GroupMemberAdd, OrgGroupCreate, OrgGroupCreateBody};
use orch_api_contract::resources::{AgentList, AgentListQuery, MachineGet};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::identity::{self, NewApiToken};
use orch_db::queries::resources::{access_fixtures as fixtures, provider_runtime, routes};
use orch_db::queries::threads::{self, Binding};
use orch_types::{Id, Json, Time, new_id};
use places::Who;
use serde_json::json;
use support::{Statements, World};

struct Key {
    id: Id,
    secret: String,
}
struct Scene {
    world: World,
    company: resources::Company,
}
impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        let company = resources::company(&world).await;
        Self { world, company }
    }
}

async fn start(world: &World, kind: ResourceKind, id: Id, owner: Id) {
    start_resource(
        &mut *world.conn().await,
        ObjectRef {
            kind: kind.into(),
            id,
        },
        owner,
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
}
async fn agent(
    world: &World,
    owner: &Who,
    home: Option<Id>,
    name: &str,
    machine: Option<Id>,
) -> Id {
    let profile = json!({"harness_version":"fixture-version","config":{"marker":name}});
    let mut conn = world.conn().await;
    let id = if let Some(home_id) = home {
        let id = new_id();
        assert!(
            routes::insert_agent(
                &mut conn,
                &routes::NewAgent {
                    id,
                    name,
                    owner_id: owner.id(),
                    home_id,
                    harness_kind: "fake",
                    machine_id: machine,
                    profile: &profile,
                },
                world.now()
            )
            .await
            .unwrap()
        );
        id
    } else {
        let id = fixtures::insert_resource(&mut conn, "agent", Some(owner.id()), None, name)
            .await
            .unwrap();
        sqlx::query("UPDATE agents SET profile=$2,machine_id=$3 WHERE id=$1")
            .bind(id)
            .bind(&profile)
            .bind(machine)
            .execute(&mut *conn)
            .await
            .unwrap();
        id
    };
    drop(conn);
    start(world, ResourceKind::Agent, id, owner.id()).await;
    id
}
fn rule(operation: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]})
}
fn inside(operation: &str, org: Id) -> Json {
    json!({"target":{"kind":"org","id":org,"reach":"subtree"},"allow":[{"operation":operation,"actions":["read"]}]})
}
fn primary(id: Id, home: Id) -> Vec<Json> {
    vec![
        rule("AgentList", "agent", id),
        rule("AgentList", "org", home),
    ]
}
async fn issue(world: &World, user: Id, rules: Option<Vec<Json>>) -> Key {
    let policy = rules.map(|rules| {
        let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        json!(scope)
    });
    let scopes = if policy.is_none() {
        vec!["*".to_owned()]
    } else {
        Vec::new()
    };
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Agent collection control",
            token_hash: &token_hash(&secret),
            scopes: &scopes,
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        policy.as_ref(),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn key(world: &World, user: Id, rules: Vec<Json>) -> Key {
    issue(world, user, Some(rules)).await
}
async fn list(world: &World, token: &str, query: &AgentListQuery) -> RawReply {
    world
        .app
        .call(AgentList, &[])
        .token(token)
        .query(query)
        .send_bytes()
        .await
}
fn shown(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn ids(rows: &Json) -> Vec<Id> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|row| serde_json::from_value(row["id"].clone()).unwrap())
        .collect()
}
fn row(rows: &Json, id: Id) -> &Json {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == json!(id))
        .expect("the selected Agent remains")
}
fn refused(reply: RawReply, status: StatusCode) {
    assert_eq!(
        reply.status,
        status,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
}
fn denied(reply: RawReply) {
    refused(reply, StatusCode::FORBIDDEN);
}
fn same_content(actual: &Json, ordinary: &Json) {
    for field in [
        "id",
        "name",
        "address",
        "machine",
        "harness",
        "harness_kind",
        "disabled",
        "providers",
        "models",
        "owner",
        "org_id",
        "home",
        "profile",
        "descriptor",
        "descriptor_overridden",
        "revision",
        "created_at",
        "updated_at",
    ] {
        assert_eq!(
            actual[field], ordinary[field],
            "intrinsic or full-source field {field}"
        );
    }
    let ordinary_permissions = ordinary["access"]["permissions"].as_array().unwrap();
    for permission in actual["access"]["permissions"].as_array().unwrap() {
        assert!(
            ordinary_permissions.contains(permission),
            "a key cannot manufacture {permission}"
        );
    }
    assert!(actual["access"]["administers"] != true || ordinary["access"]["administers"] == true);
    let ordinary_through = ordinary["access"]["through"].as_array().unwrap();
    for through in actual["access"]["through"].as_array().unwrap() {
        assert!(
            ordinary_through.contains(through),
            "a fully named key cannot invent a through source"
        );
    }
}
fn holder<'a>(agent: &'a Json, kind: &str) -> &'a Json {
    agent["access"]["through"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|through| {
            (through["kind"] == "entry" && through["holder"]["kind"] == kind)
                .then_some(&through["holder"])
        })
        .expect("the ordinary holder remains")
}
async fn no_commands(world: &World) {
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM commands")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "stored list reads never dispatch native work");
}

#[derive(Clone, Copy)]
enum ProviderAssociation {
    ThreadOnly,
    DirectAndThread,
}

async fn provider(world: &World, owner: &Who, home: Id, agent: Id, direct: bool, name: &str) -> Id {
    let mut conn = world.conn().await;
    let id = if direct {
        let id = new_id();
        assert!(
            provider_runtime::insert_subscription(
                &mut conn,
                &provider_runtime::NewSubscription {
                    id,
                    name,
                    owner_id: owner.id(),
                    home_id: home,
                    harness_kind: "fake",
                    agent_id: agent,
                    config: &json!({}),
                    arrangements: &["separated".to_owned()],
                    runtime_config: &json!({}),
                },
                world.now()
            )
            .await
            .unwrap()
        );
        id
    } else {
        fixtures::insert_resource(&mut conn, "provider", Some(owner.id()), Some(home), name)
            .await
            .unwrap()
    };
    drop(conn);
    start(world, ResourceKind::Provider, id, owner.id()).await;
    id
}
async fn model(world: &World, owner: Id, provider: Id, name: &str) {
    let mut conn = world.conn().await;
    fixtures::insert_model(&mut conn, provider, name, &["low", "high"], false, owner)
        .await
        .unwrap();
    // These are real ModelParameters members; arbitrary JSON marker members would be discarded.
    sqlx::query("UPDATE provider_models SET display_name=$3,parameters=$4,groups=ARRAY['ordinary-model-group'],source='suggested' WHERE provider_id=$1 AND model_name=$2")
        .bind(provider).bind(name).bind(format!("Full model {name}"))
        .bind(json!({"efforts":["low","high"],"default_effort":"high","context_window":8192}))
        .execute(&mut *conn).await.unwrap();
}
async fn thread_link(world: &World, owner: &Who, home: Id, agent: Id, provider: Id) -> Id {
    let project = places::make_project(world, owner, home, "Association project").await;
    let topic = places::make_topic(world, owner, project.base.id, "Association topic").await;
    let id = new_id();
    let mut conn = world.conn().await;
    assert!(
        threads::insert_thread(
            &mut conn,
            id,
            topic.base.id,
            "Associated thread",
            owner.id(),
            world.now()
        )
        .await
        .unwrap()
    );
    threads::set_binding(
        &mut conn,
        id,
        &Binding {
            agent,
            provider,
            arrangement: "separated",
            executor: None,
            tool_set: None,
        },
        0,
    )
    .await
    .unwrap();
    id
}
async fn personal_machine(world: &World, owner: Id) -> Id {
    fixtures::insert_persons_machine(&mut *world.conn().await, owner, "Private physical source")
        .await
        .unwrap()
}
async fn hosted_machine(s: &Scene) -> (Id, Id) {
    let physical = personal_machine(&s.world, s.company.boss.id()).await;
    sqlx::query("UPDATE machines SET roles=ARRAY['node','agent'] WHERE id=$1")
        .bind(physical)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let host = new_id();
    sqlx::query("INSERT INTO hosts(id,machine_id,owner_id,home_id,org_id,name) SELECT $1,$2,$3,id,org_id,'Canonical Agent host' FROM nodes WHERE id=$4")
        .bind(host).bind(physical).bind(s.company.boss.id()).bind(s.company.acme)
        .execute(&s.world.db.pool).await.unwrap();
    start(&s.world, ResourceKind::Host, host, s.company.boss.id()).await;
    (physical, host)
}

#[tokio::test]
async fn plain_agent_results_keep_intrinsic_content_and_require_the_exact_selected_read() {
    let s = Scene::new().await;
    let id = agent(
        &s.world,
        &s.company.boss,
        Some(s.company.acme),
        "Plain selected profile",
        None,
    )
    .await;
    let ordinary = shown(list(&s.world, &s.company.boss.token, &Default::default()).await);
    let legacy = issue(&s.world, s.company.boss.id(), None).await;
    assert_eq!(
        shown(list(&s.world, &legacy.secret, &Default::default()).await),
        ordinary
    );
    let token = key(&s.world, s.company.boss.id(), primary(id, s.company.acme)).await;
    let selected = shown(list(&s.world, &token.secret, &Default::default()).await);
    assert_eq!(ids(&selected), [id]);
    same_content(row(&selected, id), row(&ordinary, id));
    assert_eq!(
        row(&selected, id)["profile"]["config"]["marker"],
        "Plain selected profile"
    );
    assert_eq!(row(&selected, id)["providers"], json!([]));
    assert_eq!(row(&selected, id)["models"], json!([]));
    assert!(row(&selected, id)["machine"].is_null());
    let mut action = rule("AgentList", "agent", id);
    action["allow"][0]["actions"] = json!(["write"]);
    let mut resource = rule("AgentList", "agent", id);
    resource["resources"] = json!(["provider"]);
    for rules in [
        Vec::new(),
        vec![rule("AgentGet", "agent", id)],
        vec![action],
        vec![resource],
    ] {
        let token = key(&s.world, s.company.boss.id(), rules).await;
        denied(list(&s.world, &token.secret, &Default::default()).await);
    }
    let empty =
        places::make_department(&s.world, &s.company.boss, s.company.acme, "Empty", "empty")
            .await
            .base
            .id;
    let query = AgentListQuery {
        org_id: Some(empty),
    };
    assert!(ids(&shown(list(&s.world, &token.secret, &query).await)).is_empty());
    let none = key(&s.world, s.company.boss.id(), Vec::new()).await;
    denied(list(&s.world, &none.secret, &query).await);
    let stranger = key(
        &s.world,
        s.company.stranger.id(),
        primary(id, s.company.acme),
    )
    .await;
    assert!(
        ids(&shown(
            list(&s.world, &stranger.secret, &Default::default()).await
        ))
        .is_empty()
    );
    no_commands(&s.world).await;
}

#[tokio::test]
async fn org_system_and_minimal_hidden_agent_rows_keep_browser_and_legacy_policy() {
    let s = Scene::new().await;
    let department = places::make_department(
        &s.world,
        &s.company.boss,
        s.company.acme,
        "Research",
        "research",
    )
    .await
    .base
    .id;
    let local = agent(
        &s.world,
        &s.company.boss,
        Some(department),
        "Local profile",
        None,
    )
    .await;
    let root = agent(
        &s.world,
        &s.company.boss,
        Some(s.company.acme),
        "Other local profile",
        None,
    )
    .await;
    let system = agent(&s.world, &s.company.boss, None, "System profile", None).await;
    // System pages have no root membership through which an Owners entry counts.
    // Give this reader the explicit native View required by the existing list policy.
    resources::name_person(&s.world, "agent", system, "agent.view", s.company.boss.id()).await;
    let all = shown(list(&s.world, &s.company.boss.token, &Default::default()).await);
    assert!(
        ids(&all).contains(&local),
        "the department profile is ordinarily visible"
    );
    assert!(
        ids(&all).contains(&system),
        "the system profile has an explicit native View grant"
    );
    let legacy = issue(&s.world, s.company.boss.id(), None).await;
    let query = AgentListQuery {
        org_id: Some(department),
    };
    let ordinary = shown(list(&s.world, &s.company.boss.token, &query).await);
    assert!(
        ids(&ordinary).contains(&local),
        "the selected org includes its own profile"
    );
    assert!(
        ids(&ordinary).contains(&system),
        "an ordinarily readable system profile belongs to every org"
    );
    assert!(!ids(&ordinary).contains(&root));
    assert_eq!(
        shown(list(&s.world, &legacy.secret, &query).await),
        ordinary
    );
    let token = key(
        &s.world,
        s.company.boss.id(),
        vec![
            inside("AgentList", s.company.acme),
            rule("AgentList", "agent", system),
        ],
    )
    .await;
    let scoped = shown(list(&s.world, &token.secret, &query).await);
    assert_eq!(ids(&scoped), ids(&ordinary));
    for id in ids(&ordinary) {
        same_content(row(&scoped, id), row(&ordinary, id));
    }
    resources::name_person(
        &s.world,
        "agent",
        root,
        "access.manage",
        s.company.viewer.id(),
    )
    .await;
    // Hidden rows must not acquire dependency reads merely because they have stored associations.
    let related = provider(
        &s.world,
        &s.company.boss,
        s.company.acme,
        root,
        true,
        "Private hidden dependency",
    )
    .await;
    model(
        &s.world,
        s.company.boss.id(),
        related,
        "hidden-dependency-model",
    )
    .await;
    let ordinary = shown(list(&s.world, &s.company.viewer.token, &Default::default()).await);
    let hidden = row(&ordinary, root);
    assert_eq!(hidden["content_hidden"], true);
    assert!(hidden.get("profile").is_none() && hidden.get("models").is_none());
    let token = key(
        &s.world,
        s.company.viewer.id(),
        vec![rule("AgentList", "agent", root)],
    )
    .await;
    assert_eq!(
        row(
            &shown(list(&s.world, &token.secret, &Default::default()).await),
            root
        ),
        hidden
    );
}

#[tokio::test]
async fn selected_agent_after_the_first_five_hundred_does_not_expand_the_ordinary_window() {
    let s = Scene::new().await;
    let mut made = Vec::new();
    for index in 0..501 {
        let name = format!("Window profile {index}");
        let id = agent(&s.world, &s.company.boss, Some(s.company.acme), &name, None).await;
        made.push((id, name));
    }
    made.sort_by_key(|(id, _)| *id);
    let (late, marker) = made.last().unwrap();
    let ordinary = shown(list(&s.world, &s.company.boss.token, &Default::default()).await);
    assert_eq!(ids(&ordinary).len(), 500);
    assert!(!ids(&ordinary).contains(late));
    let legacy = issue(&s.world, s.company.boss.id(), None).await;
    assert_eq!(
        shown(list(&s.world, &legacy.secret, &Default::default()).await),
        ordinary
    );
    let token = key(
        &s.world,
        s.company.boss.id(),
        primary(*late, s.company.acme),
    )
    .await;
    let selected = shown(list(&s.world, &token.secret, &Default::default()).await);
    assert_eq!(ids(&selected), [*late]);
    assert_eq!(
        row(&selected, *late)["profile"]["config"]["marker"],
        marker.as_str()
    );
}

async fn provider_sources(association: ProviderAssociation) {
    let s = Scene::new().await;
    let id = agent(
        &s.world,
        &s.company.boss,
        Some(s.company.acme),
        "Provider-bound profile",
        None,
    )
    .await;
    resources::name_person(&s.world, "agent", id, "agent.view", s.company.viewer.id()).await;
    let direct = matches!(association, ProviderAssociation::DirectAndThread);
    let dependency = provider(
        &s.world,
        &s.company.boss,
        s.company.acme,
        id,
        direct,
        "Readable model source",
    )
    .await;
    model(
        &s.world,
        s.company.boss.id(),
        dependency,
        "full-model-marker",
    )
    .await;
    let thread = thread_link(&s.world, &s.company.boss, s.company.acme, id, dependency).await;
    let private = provider(
        &s.world,
        &s.company.boss,
        s.company.acme,
        id,
        true,
        "Ordinary unreadable model source",
    )
    .await;
    model(
        &s.world,
        s.company.boss.id(),
        private,
        "unreadable-model-marker",
    )
    .await;
    // Prove both ordinary premises before the first modern-key expectation.
    let ordinary_empty = shown(list(&s.world, &s.company.viewer.token, &Default::default()).await);
    assert_eq!(row(&ordinary_empty, id)["providers"], json!([]));
    assert_eq!(row(&ordinary_empty, id)["models"], json!([]));
    resources::name_person(
        &s.world,
        "provider",
        dependency,
        "provider.view",
        s.company.viewer.id(),
    )
    .await;
    let ordinary = shown(list(&s.world, &s.company.viewer.token, &Default::default()).await);
    let expected = row(&ordinary, id);
    assert_eq!(expected["providers"].as_array().unwrap().len(), 1);
    let models = expected["models"].as_array().unwrap();
    assert_eq!(
        models.len(),
        1,
        "direct and thread association of one Provider deduplicate"
    );
    assert_eq!(models[0]["provider_id"], json!(dependency));
    assert_eq!(models[0]["display_name"], "Full model full-model-marker");
    assert_eq!(models[0]["parameters"]["default_effort"], "high");
    assert_eq!(models[0]["parameters"]["context_window"], 8192);
    assert_eq!(models[0]["groups"], json!(["ordinary-model-group"]));
    assert_eq!(models[0]["enabled"], false);
    assert_eq!(models[0]["source"], "suggested");
    assert!(!ordinary.to_string().contains("unreadable-model-marker"));
    for extra in [
        Vec::new(),
        vec![rule("ProviderGet", "provider", dependency)],
    ] {
        let mut rules = primary(id, s.company.acme);
        rules.extend(extra);
        let token = key(&s.world, s.company.viewer.id(), rules).await;
        let response = list(&s.world, &token.secret, &Default::default()).await;
        assert!(!String::from_utf8_lossy(&response.raw).contains("full-model-marker"));
        denied(response);
    }
    let mut full_rules = primary(id, s.company.acme);
    full_rules.extend([
        rule("AgentList", "provider", dependency),
        rule("AgentList", "provider", private),
    ]);
    let token = key(&s.world, s.company.viewer.id(), full_rules).await;
    let selected = shown(list(&s.world, &token.secret, &Default::default()).await);
    same_content(row(&selected, id), expected);
    assert!(
        !selected.to_string().contains("unreadable-model-marker"),
        "key scope cannot manufacture ordinary ProviderView"
    );
    sqlx::query("UPDATE nodes SET deleted_at=$2 WHERE id=$1")
        .bind(thread)
        .bind(s.world.now())
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let after = shown(list(&s.world, &s.company.viewer.token, &Default::default()).await);
    let count = if direct { 1 } else { 0 };
    assert_eq!(
        row(&after, id)["providers"].as_array().unwrap().len(),
        count
    );
    assert_eq!(row(&after, id)["models"].as_array().unwrap().len(), count);
    if !direct {
        let empty = key(&s.world, s.company.viewer.id(), primary(id, s.company.acme)).await;
        same_content(
            row(
                &shown(list(&s.world, &empty.secret, &Default::default()).await),
                id,
            ),
            row(&after, id),
        );
    }
    no_commands(&s.world).await;
}

#[tokio::test]
async fn direct_and_thread_provider_sources_deduplicate_and_keep_full_models() {
    provider_sources(ProviderAssociation::DirectAndThread).await;
}
#[tokio::test]
async fn thread_only_provider_sources_use_live_associations_and_ordinary_provider_view() {
    provider_sources(ProviderAssociation::ThreadOnly).await;
}

#[tokio::test]
async fn personal_machine_source_preserves_agent_view_without_adding_machine_view() {
    let s = Scene::new().await;
    let machine = personal_machine(&s.world, s.company.boss.id()).await;
    let id = agent(
        &s.world,
        &s.company.boss,
        Some(s.company.acme),
        "Personal machine profile",
        Some(machine),
    )
    .await;
    resources::name_person(&s.world, "agent", id, "agent.view", s.company.viewer.id()).await;
    refused(
        s.world
            .app
            .call(MachineGet, &[&machine])
            .token(&s.company.viewer.token)
            .send_bytes()
            .await,
        StatusCode::NOT_FOUND,
    );
    let ordinary = shown(list(&s.world, &s.company.viewer.token, &Default::default()).await);
    assert_eq!(row(&ordinary, id)["machine"]["id"], json!(machine));
    for extra in [Vec::new(), vec![rule("MachineGet", "machine", machine)]] {
        let mut rules = primary(id, s.company.acme);
        rules.extend(extra);
        let token = key(&s.world, s.company.viewer.id(), rules).await;
        denied(list(&s.world, &token.secret, &Default::default()).await);
    }
    let mut rules = primary(id, s.company.acme);
    rules.push(rule("AgentList", "machine", machine));
    let token = key(&s.world, s.company.viewer.id(), rules).await;
    same_content(
        row(
            &shown(list(&s.world, &token.secret, &Default::default()).await),
            id,
        ),
        row(&ordinary, id),
    );
    sqlx::query("UPDATE machines SET deleted_at=$2 WHERE id=$1")
        .bind(machine)
        .bind(s.world.now())
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let plain = key(&s.world, s.company.viewer.id(), primary(id, s.company.acme)).await;
    let missing = shown(list(&s.world, &plain.secret, &Default::default()).await);
    assert!(
        row(&missing, id)["machine"].is_null(),
        "a deleted physical reference is factually absent"
    );
    assert_eq!(row(&missing, id)["providers"], json!([]));
    no_commands(&s.world).await;
}

#[tokio::test]
async fn hosted_machine_source_uses_host_identity_and_machine_business_family() {
    let s = Scene::new().await;
    let (machine, host) = hosted_machine(&s).await;
    let id = agent(
        &s.world,
        &s.company.boss,
        Some(s.company.acme),
        "Hosted machine profile",
        Some(machine),
    )
    .await;
    resources::name_person(&s.world, "agent", id, "agent.view", s.company.viewer.id()).await;
    refused(
        s.world
            .app
            .call(MachineGet, &[&machine])
            .token(&s.company.viewer.token)
            .send_bytes()
            .await,
        StatusCode::NOT_FOUND,
    );
    let ordinary = shown(list(&s.world, &s.company.viewer.token, &Default::default()).await);
    assert_eq!(row(&ordinary, id)["machine"]["id"], json!(machine));
    let mut canonical = rule("AgentList", "host", host);
    canonical["resources"] = json!(["machine"]);
    let mut wrong_family = canonical.clone();
    wrong_family["resources"] = json!(["host"]);
    for source in [
        rule("AgentList", "machine", machine),
        wrong_family,
        rule("MachineGet", "host", host),
    ] {
        let mut rules = primary(id, s.company.acme);
        rules.push(source);
        let token = key(&s.world, s.company.viewer.id(), rules).await;
        denied(list(&s.world, &token.secret, &Default::default()).await);
    }
    let mut rules = primary(id, s.company.acme);
    rules.push(canonical);
    let token = key(&s.world, s.company.viewer.id(), rules).await;
    same_content(
        row(
            &shown(list(&s.world, &token.secret, &Default::default()).await),
            id,
        ),
        row(&ordinary, id),
    );
    assert_ne!(machine, host);
    no_commands(&s.world).await;
}

#[tokio::test]
async fn agent_home_role_and_full_group_sources_follow_this_same_operation() {
    let s = Scene::new().await;
    let id = agent(
        &s.world,
        &s.company.boss,
        Some(s.company.acme),
        "Named-source profile",
        None,
    )
    .await;
    let project = places::make_project(
        &s.world,
        &s.company.boss,
        s.company.acme,
        "Protected project",
    )
    .await;
    let topic = places::make_topic(
        &s.world,
        &s.company.boss,
        project.base.id,
        "Protected topic",
    )
    .await;
    resources::name_on_page(
        &s.world,
        "agent",
        id,
        "agent.view",
        ("role", Some(topic.base.id), Some("viewer")),
    )
    .await;
    resources::name_person(&s.world, "agent", id, "agent.view", s.company.viewer.id()).await;
    let ordinary = shown(list(&s.world, &s.company.viewer.token, &Default::default()).await);
    let expected = row(&ordinary, id);
    assert!(
        holder(expected, "role")["at"]["name"]
            .as_str()
            .unwrap()
            .contains("Protected topic")
    );
    let nodes = [
        ("org", s.company.acme),
        ("project", project.base.id),
        ("topic", topic.base.id),
    ];
    let mut wrong = vec![rule("AgentList", "agent", id)];
    wrong.extend(nodes.iter().map(|(kind, id)| rule("AgentGet", kind, *id)));
    let token = key(&s.world, s.company.viewer.id(), wrong).await;
    let limited = shown(list(&s.world, &token.secret, &Default::default()).await);
    let hidden = row(&limited, id);
    assert!(hidden["home"]["name"].is_null() && hidden["home"]["redacted"] == true);
    assert!(holder(hidden, "role")["at"]["name"].is_null());
    for field in ["id", "name", "address", "owner", "profile", "descriptor"] {
        assert_eq!(hidden[field], expected[field]);
    }
    assert_eq!(
        hidden["access"]["permissions"],
        expected["access"]["permissions"]
    );
    assert_eq!(
        hidden["access"]["through"].as_array().unwrap().len(),
        expected["access"]["through"].as_array().unwrap().len()
    );
    let group = s
        .world
        .app
        .call(OrgGroupCreate, &[&s.company.acme])
        .token(&s.company.boss.token)
        .body(&OrgGroupCreateBody {
            name: "Agent readers".into(),
            description: Some("Actual full Agent reader group".into()),
        })
        .send()
        .await
        .ok();
    for who in [&s.company.viewer, &s.company.admin] {
        s.world
            .app
            .call(GroupMemberAdd, &[&group.id, &who.id()])
            .token(&s.company.boss.token)
            .send()
            .await
            .ok();
    }
    resources::name_on_page(
        &s.world,
        "agent",
        id,
        "agent.view",
        ("group", Some(group.id), None),
    )
    .await;
    let ordinary = shown(list(&s.world, &s.company.viewer.token, &Default::default()).await);
    let expected_group = &holder(row(&ordinary, id), "group")["group"];
    assert_eq!(expected_group["member_count"], 2);
    assert_eq!(
        expected_group["description"],
        "Actual full Agent reader group"
    );
    let mut full = vec![rule("AgentList", "agent", id)];
    full.extend(nodes.iter().map(|(kind, id)| rule("AgentList", kind, *id)));
    for extra in [Vec::new(), vec![rule("AgentGet", "group", group.id)]] {
        let mut rules = full.clone();
        rules.extend(extra);
        let token = key(&s.world, s.company.viewer.id(), rules).await;
        denied(list(&s.world, &token.secret, &Default::default()).await);
    }
    full.push(rule("AgentList", "group", group.id));
    let token = key(&s.world, s.company.viewer.id(), full).await;
    same_content(
        row(
            &shown(list(&s.world, &token.secret, &Default::default()).await),
            id,
        ),
        row(&ordinary, id),
    );
}

#[tokio::test]
async fn current_primary_and_provider_source_bounds_refuse_old_models_on_the_next_request() {
    let s = Scene::new().await;
    let id = agent(
        &s.world,
        &s.company.boss,
        Some(s.company.acme),
        "Current profile",
        None,
    )
    .await;
    let dependency = provider(
        &s.world,
        &s.company.boss,
        s.company.acme,
        id,
        true,
        "Current Provider source",
    )
    .await;
    model(
        &s.world,
        s.company.boss.id(),
        dependency,
        "current-model-marker",
    )
    .await;
    let full = vec![inside("AgentList", s.company.acme)];
    for change in ["primary", "source", "revoke", "expiry"] {
        let token = key(&s.world, s.company.boss.id(), full.clone()).await;
        assert!(
            !row(
                &shown(list(&s.world, &token.secret, &Default::default()).await),
                id
            )["models"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        match change {
            "primary" | "source" => {
                let rules = if change == "primary" {
                    Vec::new()
                } else {
                    primary(id, s.company.acme)
                };
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(token.id)
                    .bind(json!({"rules":rules}))
                    .execute(&s.world.db.pool)
                    .await
                    .unwrap();
            }
            "revoke" => {
                assert!(
                    identity::revoke_api_token(
                        &mut *s.world.conn().await,
                        token.id,
                        s.company.boss.id(),
                        s.world.now()
                    )
                    .await
                    .unwrap()
                );
            }
            _ => {
                sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                    .bind(token.id)
                    .bind(s.world.now() - chrono::Duration::seconds(1))
                    .execute(&s.world.db.pool)
                    .await
                    .unwrap();
            }
        }
        let after = list(&s.world, &token.secret, &Default::default()).await;
        assert!(!String::from_utf8_lossy(&after.raw).contains("current-model-marker"));
        refused(
            after,
            if matches!(change, "revoke" | "expiry") {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::FORBIDDEN
            },
        );
    }
    let outside = places::make_org(&s.world, &s.company.boss, "Other root", "other-root")
        .await
        .base
        .id;
    let token = key(&s.world, s.company.boss.id(), full).await;
    sqlx::query("UPDATE providers SET home_id=$2,org_id=$2 WHERE id=$1")
        .bind(dependency)
        .bind(outside)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let ordinary = shown(list(&s.world, &s.company.boss.token, &Default::default()).await);
    assert_eq!(row(&ordinary, id)["models"].as_array().unwrap().len(), 1);
    denied(list(&s.world, &token.secret, &Default::default()).await);
}

#[tokio::test]
async fn populated_provider_model_and_machine_sources_keep_batched_query_cost() {
    let s = Scene::new().await;
    async fn add(s: &Scene, index: usize) {
        let (machine, _) = hosted_machine(s).await;
        let id = agent(
            &s.world,
            &s.company.boss,
            Some(s.company.acme),
            &format!("Batch profile {index}"),
            Some(machine),
        )
        .await;
        let dependency = provider(
            &s.world,
            &s.company.boss,
            s.company.acme,
            id,
            true,
            &format!("Batch provider {index}"),
        )
        .await;
        model(
            &s.world,
            s.company.boss.id(),
            dependency,
            &format!("batch-model-{index}"),
        )
        .await;
    }
    for index in 0..2 {
        add(&s, index).await;
    }
    let token = key(
        &s.world,
        s.company.boss.id(),
        vec![inside("AgentList", s.company.acme)],
    )
    .await;
    async fn measure(s: &Scene, token: &str, expected: usize) -> usize {
        let (reply, count) = Statements::measure(list(&s.world, token, &Default::default())).await;
        let shown = shown(reply);
        assert_eq!(ids(&shown).len(), expected);
        for row in shown.as_array().unwrap() {
            assert_eq!(row["providers"].as_array().unwrap().len(), 1);
            assert_eq!(row["models"].as_array().unwrap().len(), 1);
            assert!(!row["machine"].is_null());
        }
        assert!(
            count > 0,
            "the statement observer must see this actual populated read"
        );
        count
    }
    let few = measure(&s, &token.secret, 2).await;
    for index in 2..12 {
        add(&s, index).await;
    }
    let many = measure(&s, &token.secret, 12).await;
    assert_eq!(
        few, many,
        "populated dependencies use bulk readers within the same batch"
    );
    no_commands(&s.world).await;
}

// Test-only request-local classifier: no raw causes, SQL, headers or payload values are emitted.
const AGENT_BOUNDARY_CAUSE_BYTES: usize = 4096;

#[derive(Default)]
struct AgentBoundaryCause {
    text: String,
    truncated: bool,
}
impl std::fmt::Write for AgentBoundaryCause {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        let mut count = value
            .len()
            .min(AGENT_BOUNDARY_CAUSE_BYTES.saturating_sub(self.text.len()));
        while !value.is_char_boundary(count) {
            count -= 1;
        }
        self.truncated |= count < value.len();
        self.text.push_str(&value[..count]);
        Ok(())
    }
}
impl tracing::field::Visit for AgentBoundaryCause {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "cause" {
            let _ = std::fmt::Write::write_str(self, value);
        }
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "cause" {
            let _ = std::fmt::write(self, format_args!("{value:?}"));
        }
    }
}
struct AgentBoundaryErrors {
    operation: &'static str,
    narrow: bool,
}
impl tracing::Subscriber for AgentBoundaryErrors {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "orch_api_core::middleware::requests"
            && *metadata.level() == tracing::Level::ERROR
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if !self.enabled(event.metadata()) {
            return;
        }
        let mut cause = AgentBoundaryCause::default();
        event.record(&mut cause);
        let class = if !cause.truncated && cause.text == orch_db::DbError::Retry.to_string() {
            "DbError::Retry"
        } else if cause.text == "the operation's current prepared operands are unavailable" {
            "missing_prepared_data"
        } else if cause.text.is_empty() {
            "missing_cause"
        } else {
            "other_internal"
        };
        eprintln!(
            "agent_current_boundary {}",
            serde_json::json!({
                "operation": self.operation, "narrowed": self.narrow,
                "cause_class": class, "truncated": cause.truncated,
                "cause_prefix_blake3": orch_types::Hash::of(cause.text.as_bytes()).to_string(),
            })
        );
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

// Follow-up to the sealed ten controls: retain the established C01 30-second watchdog.
// It bounds a broken rendezvous; elapsed time never proves that a request reached the reader.
const AGENT_READ_CHECKPOINT_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

// The current native snapshot's actual HTTP identity read:
// - identity::api_token_by_hash uses NO KEY UPDATE SKIP LOCKED and an ordinary SELECT fallback;
//   holding this token row does not hold authentication at the update bookkeeping step.
// - AgentList registers an optional operation without an initial callback. Registry's
//   prepare_registered_operation no-initial branch loads ordinary principal facts, and
//   agents/collection_read::prepare returns None for the opening legacy caller.
// Consequently, the current transport identity's blocked FOR SHARE is the actual
// snapshot boundary after opening. The older current_boundary lookup comes later.

async fn legacy_agent_actual_checkpoint(
    s: &Scene,
    token: &Key,
    ordinary: &Json,
    id: Id,
    marker: &str,
    narrow: bool,
) {
    use axum::body::{Body, to_bytes};
    use http::Request;
    use orch_api_contract::common::{ErrorCode, MAX_JSON_BYTES, Problem};
    use orch_api_core::registry::Registry;
    use tower::ServiceExt;

    // Lock only this fresh token, not its account: initial ordinary account reads remain free.
    let mut blocker = s.world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM api_tokens WHERE id=$1 AND user_id=$2 FOR UPDATE")
        .bind(token.id)
        .bind(s.company.boss.id())
        .execute(&mut *blocker)
        .await
        .unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();

    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = registry
        .into_router_for(orch_api_contract::surface::HttpSurface::Key)
        .with_state(s.world.app.app.clone());
    let request = Request::builder()
        .method("GET")
        .uri("/agents")
        .header("authorization", format!("Bearer {}", token.secret))
        .body(Body::empty())
        .unwrap();
    use tracing::instrument::WithSubscriber;
    let mut action = tokio::spawn(
        async move { app.oneshot(request).await.unwrap() }.with_subscriber(tracing::Dispatch::new(
            AgentBoundaryErrors {
                operation: "AgentList",
                narrow,
            },
        )),
    );
    let witness = tokio::time::timeout(AGENT_READ_CHECKPOINT_WAIT, async {
        loop {
            if action.is_finished() {
                break false;
            }
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                 WHERE datname=current_database() AND wait_event_type='Lock'
                   AND query LIKE $2 AND $1=ANY(pg_blocking_pids(pid)))",
            )
            .bind(blocker_pid)
            .bind(orch_api_core::testing::current_identity_query_pattern(
                orch_api_contract::surface::HttpSurface::Key,
            ))
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
            if waiting {
                break true;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    match witness {
        Ok(true) => {}
        Ok(false) => {
            blocker.rollback().await.unwrap();
            let response = action
                .await
                .expect("the already completed request can be joined");
            panic!(
                "unreachable actual-read checkpoint: request finished with {} before the exact credential query waited; no policy change was applied",
                response.status()
            );
        }
        Err(_) => {
            blocker.rollback().await.unwrap();
            action.abort();
            let _ = action.await;
            panic!(
                "unreachable actual-read checkpoint: the exact blocked credential query was not observed"
            );
        }
    }

    if narrow {
        let changed = sqlx::query(
            "UPDATE api_tokens SET scopes='{}'::text[],node_id=NULL,operation_scope=$2 WHERE id=$1",
        )
        .bind(token.id)
        .bind(json!(KeyScope::default()))
        .execute(&mut *blocker)
        .await
        .unwrap();
        assert_eq!(changed.rows_affected(), 1);
    }
    blocker.commit().await.unwrap();
    let response = match tokio::time::timeout(AGENT_READ_CHECKPOINT_WAIT, &mut action).await {
        Ok(response) => response.unwrap(),
        Err(_) => {
            action.abort();
            let _ = action.await;
            panic!("the released actual reader did not complete within the C01 watchdog");
        }
    };
    let status = response.status();
    let bytes = to_bytes(response.into_body(), MAX_JSON_BYTES)
        .await
        .unwrap();
    if narrow {
        let safe_code = serde_json::from_slice::<Problem>(&bytes)
            .ok()
            .map(|problem| problem.code);
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "exact-witness response code={safe_code:?}; only the request-local cause class/fingerprint is logged"
        );
        let problem: Problem = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(problem.code, ErrorCode::AuthzDenied);
        assert!(!String::from_utf8_lossy(&bytes).contains(marker));
    } else {
        assert_eq!(status, StatusCode::OK);
        let unchanged: Json = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(&unchanged, ordinary);
        assert_eq!(row(&unchanged, id)["profile"]["config"]["marker"], marker);
    }
    no_commands(&s.world).await;
}

#[tokio::test]
async fn opening_legacy_agent_read_rechecks_the_same_key_on_its_actual_snapshot() {
    let s = Scene::new().await;
    let marker = "Held actual-reader profile marker";
    let id = agent(
        &s.world,
        &s.company.boss,
        Some(s.company.acme),
        marker,
        None,
    )
    .await;
    let token = issue(&s.world, s.company.boss.id(), None).await;
    let ordinary = shown(list(&s.world, &s.company.boss.token, &Default::default()).await);
    assert_eq!(
        shown(list(&s.world, &token.secret, &Default::default()).await),
        ordinary,
        "the same valid legacy credential has the ordinary unheld read premise"
    );
    legacy_agent_actual_checkpoint(&s, &token, &ordinary, id, marker, false).await;
    legacy_agent_actual_checkpoint(&s, &token, &ordinary, id, marker, true).await;
}

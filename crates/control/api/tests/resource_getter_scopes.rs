//! Individual getters reuse native full projections while bounding their actual sources.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod places;
mod resources;
mod support;

use http::StatusCode;
use orch_access_contract::{decisions::ObjectRef, key_scope::KeyScope, permissions::ResourceKind};
use orch_api_contract::directory::{GroupMemberAdd, OrgGroupCreate, OrgGroupCreateBody};
use orch_api_contract::resources::{AgentGet, MachineGet, TemplateGet};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::identity::{self, NewApiToken};
use orch_db::queries::resources::{access_fixtures as fixtures, provider_runtime, routes};
use orch_db::queries::threads::{self, Binding};
use orch_types::{Id, Json, Time, new_id};
use places::Who;
use serde_json::json;
use support::World;

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
fn rule(operation: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]})
}
fn primary(operation: &str, kind: &str, id: Id, home: Id) -> Vec<Json> {
    vec![rule(operation, kind, id), rule(operation, "org", home)]
}
fn inside(operation: &str, org: Id) -> Json {
    json!({"target":{"kind":"org","id":org,"reach":"subtree"},"allow":[{"operation":operation,"actions":["read"]}]})
}
async fn agent_get(world: &World, token: &str, id: Id) -> RawReply {
    world
        .app
        .call(AgentGet, &[&id])
        .token(token)
        .send_bytes()
        .await
}
async fn template_get(world: &World, token: &str, id: Id) -> RawReply {
    world
        .app
        .call(TemplateGet, &[&id])
        .token(token)
        .send_bytes()
        .await
}
fn shown(stage: &str, reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{stage}: {}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn refused(stage: &str, reply: RawReply, status: StatusCode, markers: &[&str]) {
    assert_eq!(
        reply.status,
        status,
        "{stage}: {}",
        String::from_utf8_lossy(&reply.raw)
    );
    for marker in markers {
        assert!(
            !String::from_utf8_lossy(&reply.raw).contains(marker),
            "{stage} disclosed {marker}"
        );
    }
}
async fn hosted_machine(s: &Scene) -> (Id, Id) {
    let physical = personal_machine(&s.world, s.company.boss.id()).await;
    sqlx::query("UPDATE machines SET roles=ARRAY['node','agent'] WHERE id=$1")
        .bind(physical)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let host = new_id();
    sqlx::query("INSERT INTO hosts(id,machine_id,owner_id,home_id,org_id,name) SELECT $1,$2,$3,id,org_id,'Canonical getter host' FROM nodes WHERE id=$4")
 .bind(host).bind(physical).bind(s.company.boss.id()).bind(s.company.acme).execute(&s.world.db.pool).await.unwrap();
    start(&s.world, ResourceKind::Host, host, s.company.boss.id()).await;
    (physical, host)
}
async fn ordinary_agent(s: &Scene, who: &Who, id: Id) -> Json {
    let browser = shown(
        "ordinary browser AgentGet",
        agent_get(&s.world, &who.token, id).await,
    );
    let legacy = issue(&s.world, who.id(), None).await;
    assert_eq!(
        shown(
            "ordinary legacy AgentGet",
            agent_get(&s.world, &legacy.secret, id).await
        ),
        browser
    );
    browser
}
async fn ordinary_template(s: &Scene, who: &Who, id: Id) -> Json {
    let browser = shown(
        "ordinary browser TemplateGet",
        template_get(&s.world, &who.token, id).await,
    );
    let legacy = issue(&s.world, who.id(), None).await;
    assert_eq!(
        shown(
            "ordinary legacy TemplateGet",
            template_get(&s.world, &legacy.secret, id).await
        ),
        browser
    );
    browser
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
async fn template(world: &World, owner: &Who, home: Option<Id>, name: &str, kind: &str) -> Id {
    let mut conn = world.conn().await;
    let id = fixtures::insert_resource(&mut conn, "template", Some(owner.id()), home, name)
        .await
        .unwrap();
    sqlx::query("UPDATE env_templates SET kind=$2,spec=$3 WHERE id=$1")
        .bind(id)
        .bind(kind)
        .bind(json!({"image":"fixture:stored","marker":name}))
        .execute(&mut *conn)
        .await
        .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Template.into(),
            id,
        },
        owner.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    id
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
            name: "Resource getter control",
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
    assert_eq!(count, 0, "stored getter reads never dispatch native work");
}

#[tokio::test]
async fn exact_agent_get_keeps_intrinsic_content_and_rejects_other_operations_or_families() {
    let s = Scene::new().await;
    let id = agent(
        &s.world,
        &s.company.boss,
        Some(s.company.acme),
        "Plain getter profile",
        None,
    )
    .await;
    let normal = ordinary_agent(&s, &s.company.boss, id).await;
    refused(
        "ordinary AgentGet stranger remains403",
        agent_get(&s.world, &s.company.stranger.token, id).await,
        StatusCode::FORBIDDEN,
        &[],
    );
    assert_eq!(
        normal["profile"]["config"]["marker"],
        "Plain getter profile"
    );
    assert_eq!(normal["providers"], json!([]));
    assert_eq!(normal["models"], json!([]));
    assert!(normal["machine"].is_null());
    let token = key(
        &s.world,
        s.company.boss.id(),
        primary("AgentGet", "agent", id, s.company.acme),
    )
    .await;
    same_content(
        &shown(
            "modern exact AgentGet",
            agent_get(&s.world, &token.secret, id).await,
        ),
        &normal,
    );
    let mut action = rule("AgentGet", "agent", id);
    action["allow"][0]["actions"] = json!(["write"]);
    let mut family = rule("AgentGet", "agent", id);
    family["resources"] = json!(["template"]);
    for rules in [
        Vec::new(),
        vec![rule("AgentList", "agent", id)],
        vec![rule("AgentGet", "agent", new_id())],
        vec![action],
        vec![family],
    ] {
        let token = key(&s.world, s.company.boss.id(), rules).await;
        refused(
            "wrong AgentGet bounds",
            agent_get(&s.world, &token.secret, id).await,
            StatusCode::FORBIDDEN,
            &["Plain getter profile"],
        );
    }
    let stranger = key(
        &s.world,
        s.company.stranger.id(),
        primary("AgentGet", "agent", id, s.company.acme),
    )
    .await;
    refused(
        "scope grants no native AgentView",
        agent_get(&s.world, &stranger.secret, id).await,
        StatusCode::FORBIDDEN,
        &[],
    );
    no_commands(&s.world).await;
}

async fn provider_payloads(direct: bool) {
    let s = Scene::new().await;
    let id = agent(
        &s.world,
        &s.company.boss,
        Some(s.company.acme),
        "Getter provider profile",
        None,
    )
    .await;
    resources::name_person(&s.world, "agent", id, "agent.view", s.company.viewer.id()).await;
    let source = provider(
        &s.world,
        &s.company.boss,
        s.company.acme,
        id,
        direct,
        "Readable getter Provider",
    )
    .await;
    model(&s.world, s.company.boss.id(), source, "getter-full-model").await;
    let link = thread_link(&s.world, &s.company.boss, s.company.acme, id, source).await;
    let private = provider(
        &s.world,
        &s.company.boss,
        s.company.acme,
        id,
        true,
        "Private getter Provider",
    )
    .await;
    model(
        &s.world,
        s.company.boss.id(),
        private,
        "getter-private-model",
    )
    .await;
    let empty = ordinary_agent(&s, &s.company.viewer, id).await;
    assert_eq!(empty["providers"], json!([]));
    assert_eq!(empty["models"], json!([]));
    resources::name_person(
        &s.world,
        "provider",
        source,
        "provider.view",
        s.company.viewer.id(),
    )
    .await;
    let normal = ordinary_agent(&s, &s.company.viewer, id).await;
    assert_eq!(normal["providers"].as_array().unwrap().len(), 1);
    let models = normal["models"].as_array().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["provider_id"], json!(source));
    assert_eq!(models[0]["display_name"], "Full model getter-full-model");
    assert_eq!(models[0]["parameters"]["context_window"], 8192);
    assert_eq!(models[0]["parameters"]["default_effort"], "high");
    assert_eq!(models[0]["groups"], json!(["ordinary-model-group"]));
    assert_eq!(models[0]["enabled"], false);
    assert_eq!(models[0]["source"], "suggested");
    assert!(!normal.to_string().contains("getter-private-model"));
    for extra in [
        Vec::new(),
        vec![rule("ProviderGet", "provider", source)],
        vec![rule("AgentList", "provider", source)],
        vec![rule("AgentGet", "provider", new_id())],
    ] {
        let mut rules = primary("AgentGet", "agent", id, s.company.acme);
        rules.extend(extra);
        let token = key(&s.world, s.company.viewer.id(), rules).await;
        refused(
            "AgentGet full Provider source",
            agent_get(&s.world, &token.secret, id).await,
            StatusCode::FORBIDDEN,
            &["getter-full-model", "Readable getter Provider"],
        );
    }
    let mut rules = primary("AgentGet", "agent", id, s.company.acme);
    rules.extend([
        rule("AgentGet", "provider", source),
        rule("AgentGet", "provider", private),
    ]);
    let token = key(&s.world, s.company.viewer.id(), rules).await;
    let full = shown(
        "modern AgentGet with actual Provider source",
        agent_get(&s.world, &token.secret, id).await,
    );
    same_content(&full, &normal);
    assert!(!full.to_string().contains("getter-private-model"));
    sqlx::query("UPDATE nodes SET deleted_at=$2 WHERE id=$1")
        .bind(link)
        .bind(s.world.now())
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let after = ordinary_agent(&s, &s.company.viewer, id).await;
    assert_eq!(
        after["models"].as_array().unwrap().len(),
        if direct { 1 } else { 0 }
    );
    if !direct {
        let plain = key(
            &s.world,
            s.company.viewer.id(),
            primary("AgentGet", "agent", id, s.company.acme),
        )
        .await;
        same_content(
            &shown(
                "factually absent Provider source",
                agent_get(&s.world, &plain.secret, id).await,
            ),
            &after,
        );
    }
    no_commands(&s.world).await;
}
#[tokio::test]
async fn agent_get_direct_and_thread_sources_deduplicate_full_models() {
    provider_payloads(true).await;
}
#[tokio::test]
async fn agent_get_thread_only_sources_follow_live_association_and_ordinary_provider_view() {
    provider_payloads(false).await;
}

async fn machine_payload(hosted: bool) {
    let s = Scene::new().await;
    let (machine, host) = if hosted {
        let (machine, host) = hosted_machine(&s).await;
        (machine, Some(host))
    } else {
        (personal_machine(&s.world, s.company.boss.id()).await, None)
    };
    let id = agent(
        &s.world,
        &s.company.boss,
        Some(s.company.acme),
        "Getter physical profile",
        Some(machine),
    )
    .await;
    resources::name_person(&s.world, "agent", id, "agent.view", s.company.viewer.id()).await;
    refused(
        "ordinary MachineView intentionally absent",
        s.world
            .app
            .call(MachineGet, &[&machine])
            .token(&s.company.viewer.token)
            .send_bytes()
            .await,
        StatusCode::NOT_FOUND,
        &[],
    );
    let normal = ordinary_agent(&s, &s.company.viewer, id).await;
    assert_eq!(normal["machine"]["id"], json!(machine));
    let mut source = rule(
        "AgentGet",
        if hosted { "host" } else { "machine" },
        host.unwrap_or(machine),
    );
    source["resources"] = json!(["machine"]);
    let mut wrong_family = source.clone();
    wrong_family["resources"] = json!(["host"]);
    let mut wrong_operation = source.clone();
    wrong_operation["allow"][0]["operation"] = json!("MachineGet");
    let mut wrongs = vec![Vec::new(), vec![wrong_operation], vec![wrong_family]];
    if hosted {
        wrongs.push(vec![rule("AgentGet", "machine", machine)]);
    }
    for extra in wrongs {
        let mut rules = primary("AgentGet", "agent", id, s.company.acme);
        rules.extend(extra);
        let token = key(&s.world, s.company.viewer.id(), rules).await;
        refused(
            "AgentGet exact Machine subject",
            agent_get(&s.world, &token.secret, id).await,
            StatusCode::FORBIDDEN,
            &["Getter physical profile"],
        );
    }
    let mut rules = primary("AgentGet", "agent", id, s.company.acme);
    rules.push(source);
    let token = key(&s.world, s.company.viewer.id(), rules).await;
    same_content(
        &shown(
            "modern AgentGet Machine source without extra native View",
            agent_get(&s.world, &token.secret, id).await,
        ),
        &normal,
    );
    if !hosted {
        sqlx::query("UPDATE machines SET deleted_at=$2 WHERE id=$1")
            .bind(machine)
            .bind(s.world.now())
            .execute(&s.world.db.pool)
            .await
            .unwrap();
        let normal = ordinary_agent(&s, &s.company.viewer, id).await;
        assert!(normal["machine"].is_null());
        let token = key(
            &s.world,
            s.company.viewer.id(),
            primary("AgentGet", "agent", id, s.company.acme),
        )
        .await;
        assert!(
            shown(
                "native absent Machine reference",
                agent_get(&s.world, &token.secret, id).await
            )["machine"]
                .is_null()
        );
    }
    no_commands(&s.world).await;
}
#[tokio::test]
async fn agent_get_personal_machine_source_does_not_add_native_machine_view() {
    machine_payload(false).await;
}
#[tokio::test]
async fn agent_get_host_facade_uses_actual_host_identity_and_machine_business_family() {
    machine_payload(true).await;
}

async fn named_resource(agent_kind: bool, group_only: bool) {
    let s = Scene::new().await;
    let kind = if agent_kind { "agent" } else { "template" };
    let op = if agent_kind {
        "AgentGet"
    } else {
        "TemplateGet"
    };
    let wrong_op = if agent_kind {
        "AgentList"
    } else {
        "TemplateList"
    };
    let id = if agent_kind {
        agent(
            &s.world,
            &s.company.boss,
            Some(s.company.acme),
            "Getter named profile",
            None,
        )
        .await
    } else {
        template(
            &s.world,
            &s.company.boss,
            Some(s.company.acme),
            "Getter own recipe",
            "environment",
        )
        .await
    };
    let project = places::make_project(
        &s.world,
        &s.company.boss,
        s.company.acme,
        "Getter protected project",
    )
    .await;
    let topic = places::make_topic(
        &s.world,
        &s.company.boss,
        project.base.id,
        "Getter protected topic",
    )
    .await;
    resources::name_on_page(
        &s.world,
        kind,
        id,
        &format!("{kind}.view"),
        ("role", Some(topic.base.id), Some("viewer")),
    )
    .await;
    resources::name_person(
        &s.world,
        kind,
        id,
        &format!("{kind}.view"),
        s.company.viewer.id(),
    )
    .await;
    let normal = if agent_kind {
        ordinary_agent(&s, &s.company.viewer, id).await
    } else {
        ordinary_template(&s, &s.company.viewer, id).await
    };
    assert!(
        holder(&normal, "role")["at"]["name"]
            .as_str()
            .unwrap()
            .contains("Getter protected topic")
    );
    if !agent_kind {
        assert_eq!(
            normal["spec"],
            json!({"image":"fixture:stored","marker":"Getter own recipe"})
        );
    }
    let nodes = [
        ("org", s.company.acme),
        ("project", project.base.id),
        ("topic", topic.base.id),
    ];
    if !group_only {
        let mut rules = vec![rule(op, kind, id)];
        rules.extend(nodes.iter().map(|(kind, id)| rule(wrong_op, kind, *id)));
        let token = key(&s.world, s.company.viewer.id(), rules).await;
        let raw = if agent_kind {
            agent_get(&s.world, &token.secret, id).await
        } else {
            template_get(&s.world, &token.secret, id).await
        };
        let hidden = shown(
            "modern getter retains primary with bounded optional names",
            raw,
        );
        assert!(hidden["home"]["name"].is_null() && hidden["home"]["redacted"] == true);
        assert!(holder(&hidden, "role")["at"]["name"].is_null());
        assert!(holder(&hidden, "role")["place"].is_null());
        for field in [
            "id",
            "name",
            "address",
            "owner",
            if agent_kind { "profile" } else { "spec" },
        ] {
            assert_eq!(hidden[field], normal[field], "intrinsic {field}");
        }
        assert_eq!(
            hidden["access"]["permissions"],
            normal["access"]["permissions"]
        );
        assert_eq!(
            hidden["access"]["through"].as_array().unwrap().len(),
            normal["access"]["through"].as_array().unwrap().len()
        );
        let mut rules = vec![rule(op, kind, id)];
        rules.extend(nodes.iter().map(|(kind, id)| rule(op, kind, *id)));
        let token = key(&s.world, s.company.viewer.id(), rules).await;
        let raw = if agent_kind {
            agent_get(&s.world, &token.secret, id).await
        } else {
            template_get(&s.world, &token.secret, id).await
        };
        assert_eq!(
            shown("own-operation node names restore native getter", raw),
            normal
        );
    }
    // The existing through-source is a FULL Group payload, not merely a redacted label.
    let group = s
        .world
        .app
        .call(OrgGroupCreate, &[&s.company.acme])
        .token(&s.company.boss.token)
        .body(&OrgGroupCreateBody {
            name: "Getter readers".into(),
            description: Some("Getter full group source".into()),
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
        kind,
        id,
        &format!("{kind}.view"),
        ("group", Some(group.id), None),
    )
    .await;
    let normal = if agent_kind {
        ordinary_agent(&s, &s.company.viewer, id).await
    } else {
        ordinary_template(&s, &s.company.viewer, id).await
    };
    assert_eq!(holder(&normal, "group")["group"]["member_count"], 2);
    assert_eq!(
        holder(&normal, "group")["group"]["description"],
        "Getter full group source"
    );
    let mut base = vec![rule(op, kind, id)];
    base.extend(nodes.iter().map(|(kind, id)| rule(op, kind, *id)));
    for extra in [Vec::new(), vec![rule(wrong_op, "group", group.id)]] {
        let mut rules = base.clone();
        rules.extend(extra);
        let token = key(&s.world, s.company.viewer.id(), rules).await;
        let raw = if agent_kind {
            agent_get(&s.world, &token.secret, id).await
        } else {
            template_get(&s.world, &token.secret, id).await
        };
        refused(
            "full Group content requires same-operation source",
            raw,
            StatusCode::FORBIDDEN,
            &["Getter full group source"],
        );
    }
    base.push(rule(op, "group", group.id));
    let token = key(&s.world, s.company.viewer.id(), base).await;
    let raw = if agent_kind {
        agent_get(&s.world, &token.secret, id).await
    } else {
        template_get(&s.world, &token.secret, id).await
    };
    assert_eq!(
        shown("same-operation Group source restores native getter", raw),
        normal
    );
    no_commands(&s.world).await;
}
#[tokio::test]
async fn agent_get_bounds_optional_place_names_and_actual_full_group_payload() {
    named_resource(true, false).await;
}
#[tokio::test]
async fn template_get_keeps_intrinsic_recipe_while_bounding_places_and_actual_full_group() {
    named_resource(false, false).await;
}

#[tokio::test]
async fn template_get_actual_full_group_source_is_independent_of_optional_place_names() {
    named_resource(false, true).await;
}

#[tokio::test]
async fn agent_get_uses_current_key_and_current_source_home_on_each_request() {
    let s = Scene::new().await;
    let id = agent(
        &s.world,
        &s.company.boss,
        Some(s.company.acme),
        "Current getter profile",
        None,
    )
    .await;
    let source = provider(
        &s.world,
        &s.company.boss,
        s.company.acme,
        id,
        true,
        "Current getter source",
    )
    .await;
    model(
        &s.world,
        s.company.boss.id(),
        source,
        "current-getter-model",
    )
    .await;
    let normal = ordinary_agent(&s, &s.company.boss, id).await;
    assert_eq!(normal["models"].as_array().unwrap().len(), 1);
    for change in ["primary", "source", "revoke", "expiry"] {
        let token = key(
            &s.world,
            s.company.boss.id(),
            vec![inside("AgentGet", s.company.acme)],
        )
        .await;
        shown(
            "current AgentGet positive before key change",
            agent_get(&s.world, &token.secret, id).await,
        );
        match change {
            "primary" | "source" => {
                let rules = if change == "primary" {
                    Vec::new()
                } else {
                    primary("AgentGet", "agent", id, s.company.acme)
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
        refused(
            "current AgentGet bounds after key change",
            agent_get(&s.world, &token.secret, id).await,
            if matches!(change, "revoke" | "expiry") {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::FORBIDDEN
            },
            &["current-getter-model"],
        );
    }
    let token = key(
        &s.world,
        s.company.boss.id(),
        vec![inside("AgentGet", s.company.acme)],
    )
    .await;
    let outside = places::make_org(
        &s.world,
        &s.company.boss,
        "Getter outside source",
        "getter-outside",
    )
    .await
    .base
    .id;
    sqlx::query("UPDATE providers SET home_id=$2,org_id=$2 WHERE id=$1")
        .bind(source)
        .bind(outside)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let current = ordinary_agent(&s, &s.company.boss, id).await;
    assert_eq!(current["models"].as_array().unwrap().len(), 1);
    refused(
        "current actual Provider home",
        agent_get(&s.world, &token.secret, id).await,
        StatusCode::FORBIDDEN,
        &["current-getter-model"],
    );
    no_commands(&s.world).await;
}

#[tokio::test]
async fn template_get_primary_is_already_supported_and_still_uses_current_home_bounds() {
    let s = Scene::new().await;
    let id = template(
        &s.world,
        &s.company.boss,
        Some(s.company.acme),
        "Current getter recipe",
        "environment",
    )
    .await;
    let normal = ordinary_template(&s, &s.company.boss, id).await;
    let token = key(
        &s.world,
        s.company.boss.id(),
        vec![inside("TemplateGet", s.company.acme)],
    )
    .await;
    let full = shown(
        "already-supported modern TemplateGet primary",
        template_get(&s.world, &token.secret, id).await,
    );
    for field in ["id", "name", "spec", "home", "driver", "kind"] {
        assert_eq!(full[field], normal[field]);
    }
    let outside = places::make_org(
        &s.world,
        &s.company.boss,
        "Getter outside recipe",
        "getter-outside-recipe",
    )
    .await
    .base
    .id;
    sqlx::query("UPDATE env_templates SET home_id=$2,org_id=$2 WHERE id=$1")
        .bind(id)
        .bind(outside)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let native = ordinary_template(&s, &s.company.boss, id).await;
    assert_eq!(native["spec"], normal["spec"]);
    refused(
        "existing TemplateGet current primary home",
        template_get(&s.world, &token.secret, id).await,
        StatusCode::FORBIDDEN,
        &["Current getter recipe"],
    );
    let exact = key(
        &s.world,
        s.company.boss.id(),
        vec![rule("TemplateGet", "template", id)],
    )
    .await;
    let hidden = shown(
        "same Template identity with optional new home hidden",
        template_get(&s.world, &exact.secret, id).await,
    );
    assert_eq!(hidden["spec"], normal["spec"]);
    assert!(hidden["home"]["name"].is_null());
    no_commands(&s.world).await;
}

async fn restricted_legacy_key(world: &World, user: Id, permission: &str, node: Option<Id>) -> Key {
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Restricted legacy getter control",
            token_hash: &token_hash(&secret),
            scopes: &[permission.to_owned()],
            node_id: node,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        None,
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn restricted_legacy(agent_kind: bool) {
    let s = Scene::new().await;
    let kind = if agent_kind { "agent" } else { "template" };
    let permission = format!("{kind}.view");
    let id = if agent_kind {
        agent(
            &s.world,
            &s.company.boss,
            Some(s.company.acme),
            "Restricted legacy getter",
            None,
        )
        .await
    } else {
        template(
            &s.world,
            &s.company.boss,
            Some(s.company.acme),
            "Restricted legacy getter",
            "environment",
        )
        .await
    };
    let normal = if agent_kind {
        ordinary_agent(&s, &s.company.boss, id).await
    } else {
        ordinary_template(&s, &s.company.boss, id).await
    };
    let scalar = restricted_legacy_key(
        &s.world,
        s.company.boss.id(),
        &permission,
        Some(s.company.acme),
    )
    .await;
    let unbound = restricted_legacy_key(&s.world, s.company.boss.id(), &permission, None).await;
    let raw = if agent_kind {
        agent_get(&s.world, &unbound.secret, id).await
    } else {
        template_get(&s.world, &unbound.secret, id).await
    };
    let actual = shown(
        "restricted unbound legacy permission valid native getter",
        raw,
    );
    for field in [
        "id",
        "name",
        "home",
        if agent_kind { "profile" } else { "spec" },
    ] {
        assert_eq!(actual[field], normal[field]);
    }
    assert_eq!(actual["access"]["permissions"], json!([permission]));
    // A direct resource request has no node path in the historical legacy
    // authorizer. Its home supplies page defaults, not legacy scalar reach.
    // Existing node-context token controls cover org place refusals separately.
    let raw = if agent_kind {
        agent_get(&s.world, &scalar.secret, id).await
    } else {
        template_get(&s.world, &scalar.secret, id).await
    };
    refused(
        "original legacy scalar refuses direct resource even with matching home",
        raw,
        StatusCode::FORBIDDEN,
        &[],
    );
    let outside = places::make_org(
        &s.world,
        &s.company.boss,
        "Restricted getter outside",
        "restricted-getter-outside",
    )
    .await
    .base
    .id;
    if agent_kind {
        sqlx::query("UPDATE agents SET home_id=$2,org_id=$2 WHERE id=$1")
            .bind(id)
            .bind(outside)
            .execute(&s.world.db.pool)
            .await
            .unwrap();
    } else {
        sqlx::query("UPDATE env_templates SET home_id=$2,org_id=$2 WHERE id=$1")
            .bind(id)
            .bind(outside)
            .execute(&s.world.db.pool)
            .await
            .unwrap();
    }
    let raw = if agent_kind {
        agent_get(&s.world, &s.company.boss.token, id).await
    } else {
        template_get(&s.world, &s.company.boss.token, id).await
    };
    shown("ordinary owner still reads moved native resource", raw);
    let raw = if agent_kind {
        agent_get(&s.world, &scalar.secret, id).await
    } else {
        template_get(&s.world, &scalar.secret, id).await
    };
    // This remains a scalar compatibility refusal; it is not evidence that
    // legacy resources acquire modern current-home subtree semantics.
    refused(
        "original legacy scalar still refuses moved direct resource",
        raw,
        StatusCode::FORBIDDEN,
        &[],
    );
    no_commands(&s.world).await;
}
#[tokio::test]
async fn agent_get_keeps_restricted_legacy_permission_and_native_scalar_bounds() {
    restricted_legacy(true).await;
}
#[tokio::test]
async fn template_get_keeps_restricted_legacy_permission_and_native_scalar_bounds() {
    restricted_legacy(false).await;
}

//! Provider setup keeps its ordinary roots, filters actual offers by current source scope, and never starts work.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod resources;
mod support;

use axum::body::{Body, to_bytes};
use http::{Request, StatusCode};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyScope;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::common::{ErrorCode, Problem};
use orch_api_contract::resources::{ProviderSetupGet, ProviderSetupGetQuery};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::registry::Registry;
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::identity::{self, NewApiToken};
use orch_db::queries::resources::access_fixtures;
use orch_settings::Target;
use orch_types::{Id, Json, Time, new_id};
use places::Who;
use serde_json::json;
use std::time::Duration;
use support::World;
use tower::ServiceExt;

const PRIVATE_PROFILE: &str = "private-profile-environment-sentinel-742";
const PRIVATE_TEMPLATE: &str = "private-template-configuration-sentinel-396";
const PRIVATE_CREDENTIAL: &str = "provider-secret-value-sentinel-825";
// Reuse the established C01/C02 scheduling watchdog and JSON-body bound, never authority by time.
const LOCK_WAIT: Duration = Duration::from_secs(30);
const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;

struct Key {
    id: Id,
    user: Id,
    secret: String,
}
struct Scene {
    world: World,
    company: resources::Company,
    provider: Id,
    other: Id,
}

impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        let company = resources::company(&world).await;
        let provider = resource(
            &world,
            "provider",
            &company.boss,
            company.acme,
            "Existing setup provider",
        )
        .await;
        let other = resource(
            &world,
            "provider",
            &company.boss,
            company.acme,
            "Other setup provider",
        )
        .await;
        Self {
            world,
            company,
            provider,
            other,
        }
    }
    fn query(&self, existing: bool) -> ProviderSetupGetQuery {
        ProviderSetupGetQuery {
            org_id: self.company.acme,
            provider_id: existing.then_some(self.provider),
        }
    }
    fn root(&self, existing: bool) -> Json {
        root(
            if existing { "provider" } else { "org" },
            if existing {
                self.provider
            } else {
                self.company.acme
            },
        )
    }
}

async fn resource(world: &World, kind: &str, owner: &Who, home: Id, name: &str) -> Id {
    let mut conn = world.conn().await;
    let id = access_fixtures::insert_resource(&mut conn, kind, Some(owner.id()), Some(home), name)
        .await
        .unwrap();
    let kind = match kind {
        "provider" => ResourceKind::Provider,
        "agent" => ResourceKind::Agent,
        "template" => ResourceKind::Template,
        "host" => ResourceKind::Host,
        _ => unreachable!(),
    };
    start_resource(
        &mut conn,
        ObjectRef {
            kind: kind.into(),
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
fn root(kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":["provider"],"allow":[{"operation":"ProviderSetupGet","actions":["manage"]}]})
}
fn read(operation: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]})
}
async fn key(world: &World, user: Id, rules: Vec<Json>) -> Key {
    issue(world, user, Some(rules)).await
}
async fn legacy(world: &World, user: Id) -> Key {
    issue(world, user, None).await
}
async fn issue(world: &World, user: Id, rules: Option<Vec<Json>>) -> Key {
    let policy = rules.map(|rules| {
        let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        serde_json::to_value(scope).unwrap()
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
            name: "Provider setup scope control",
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
    Key { id, user, secret }
}
async fn setup(world: &World, token: &str, query: &ProviderSetupGetQuery) -> RawReply {
    world
        .app
        .call(ProviderSetupGet, &[])
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
fn denied(reply: RawReply) {
    assert_eq!(
        reply.status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
}
fn contains(rows: &Json, field: &str, id: Id) -> bool {
    rows.as_array()
        .is_some_and(|rows| rows.iter().any(|row| row[field] == json!(id)))
}
fn agent_present(page: &Json, id: Id) -> bool {
    page["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["agent"]["id"] == json!(id))
}
fn native_descriptor(kind: &str) -> Json {
    json!({"kind":kind,"adapter_version":1,"harness":{"name":"Useful native schema","version":"1.0.0","supported_range":">=1.0.0","status":"supported"},
        "arrangements":[{"type":"separated"}],"native_provider":{"harness_kind":kind,"title":"Useful plan schema",
            "login_methods":[{"id":"token","title":"Bring a token","summary":"An existing import method.","kind":"import","fields":[],"yields":"token","lifetime_secs":null,"requires":null,"recommended":true}],
            "options_schema":{"type":"object","properties":{"plan":{"type":"string","enum":["pro","max"]}},"additionalProperties":false},"arrangements":["separated"]},
        "forwarded_protocols":["openai-responses"],"direct_protocols":[]})
}
async fn agent(s: &Scene, name: &str, kind: &str, descriptor: Json) -> Id {
    let id = resource(&s.world, "agent", &s.company.boss, s.company.acme, name).await;
    sqlx::query("UPDATE agents SET harness_kind=$2,descriptor=$3,profile=$4 WHERE id=$1")
        .bind(id)
        .bind(kind)
        .bind(descriptor)
        .bind(json!({"env":{"PRIVATE":PRIVATE_PROFILE},"config":{"private":PRIVATE_PROFILE}}))
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    id
}
fn source_descriptor(name: &str) -> Json {
    let mut descriptor = orch_provider_contract::SourceDescriptor::new(
        name,
        "Useful source schema",
        "A stored program declaration.",
    );
    descriptor.facts = Some(Default::default());
    serde_json::to_value(descriptor).unwrap()
}
async fn machine(s: &Scene, owner: &Who, name: &str, program: &str) -> Id {
    let mut conn = s.world.conn().await;
    let id = access_fixtures::insert_persons_machine(&mut conn, owner.id(), name)
        .await
        .unwrap();
    let policy = orch_link_contract::policy::LocalPolicy {
        provider_upstreams: vec!["https://permitted.example/v1".into()],
        ..Default::default()
    };
    sqlx::query("UPDATE machines SET roles=ARRAY['provider'],facts=$2 WHERE id=$1").bind(id)
        .bind(json!({"policy":policy,"plugins":[{"kind":"provider","name":program,"descriptor":source_descriptor(program)}]})).execute(&mut *conn).await.unwrap();
    id
}
async fn template(s: &Scene, name: &str, program: &str, kind: &str, enabled: bool) -> Id {
    let id = resource(&s.world, "template", &s.company.boss, s.company.acme, name).await;
    sqlx::query("UPDATE env_templates SET kind=$2,enabled=$3,spec=$4 WHERE id=$1").bind(id).bind(kind).bind(enabled)
        .bind(json!({"plugins":[{"kind":"provider","name":program,"command":["/opt/useful-plan-reader","--json"]}],"private":PRIVATE_TEMPLATE})).execute(&s.world.db.pool).await.unwrap();
    id
}
async fn desired_template(s: &Scene, template: Id) {
    // Provider own values use the ordinary whole-map PATCH with its current revision.
    // Preserve every unrelated value and act as the actual provider owner.
    let (mut config, revision): (Json, i64) =
        sqlx::query_as("SELECT runtime_config,revision FROM providers WHERE id=$1")
            .bind(s.provider)
            .fetch_one(&s.world.db.pool)
            .await
            .unwrap();
    config
        .as_object_mut()
        .unwrap()
        .insert("runtime.template_id".to_owned(), json!(template));
    let reply = s
        .world
        .app
        .call(orch_api_contract::resources::ProviderUpdate, &[&s.provider])
        .token(&s.company.boss.token)
        .body(&orch_api_contract::resources::ProviderUpdateBody {
            name: None,
            upstreams: None,
            config: None,
            arrangements: None,
            runtime_config: Some(config),
            expected_revision: revision,
        })
        .send_bytes()
        .await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "the ordinary owner stores the current desired template: {}",
        String::from_utf8_lossy(&reply.raw)
    );
}
fn source<'a>(page: &'a Json, name: &str) -> Option<&'a Json> {
    page["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["name"] == name)
}
fn machine_present(page: &Json, id: Id) -> bool {
    page["machines"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| row["machine"]["id"] == json!(id)))
}
async fn facts(world: &World) -> Json {
    sqlx::query_scalar("SELECT jsonb_build_object('commands',(SELECT count(*) FROM commands),'jobs',(SELECT count(*) FROM jobs),'secrets',(SELECT COALESCE(jsonb_agg(jsonb_build_array(id,encode(sealed,'hex')) ORDER BY id),'[]'::jsonb) FROM secrets))")
        .fetch_one(&world.db.pool).await.unwrap()
}

#[tokio::test]
async fn existing_setup_requires_its_exact_manage_provider_and_current_home_without_creation_rights()
 {
    let s = Scene::new().await;
    resources::name_person(
        &s.world,
        "provider",
        s.provider,
        "provider.manage",
        s.company.viewer.id(),
    )
    .await;
    s.world
        .set_at(
            Target::Node(s.company.acme),
            "resources.created_by_members",
            json!(["host"]),
        )
        .await;
    let query = s.query(true);
    let ordinary = shown(setup(&s.world, &s.company.viewer.token, &query).await);
    assert!(!ordinary["protocols"].as_array().unwrap().is_empty());
    let actual = key(&s.world, s.company.viewer.id(), vec![s.root(true)]).await;
    let scoped = shown(setup(&s.world, &actual.secret, &query).await);
    let public_protocols = |page: &Json| {
        page["protocols"].as_array().unwrap().iter()
            .map(|protocol| json!({"name":protocol["name"],"title":protocol["title"],"endpoint_options":protocol["endpoint_options"]}))
            .collect::<Vec<_>>()
    };
    assert_eq!(public_protocols(&scoped), public_protocols(&ordinary));
    let legacy = legacy(&s.world, s.company.viewer.id()).await;
    assert_eq!(
        shown(setup(&s.world, &legacy.secret, &query).await),
        ordinary
    );
    let wrong_target = key(
        &s.world,
        s.company.viewer.id(),
        vec![root("provider", s.other)],
    )
    .await;
    let wrong_op=key(&s.world,s.company.viewer.id(),vec![json!({"target":{"kind":"provider","id":s.provider,"reach":"self"},"allow":[{"operation":"ProviderUpdate","actions":["manage"]}]})]).await;
    let read_only = key(
        &s.world,
        s.company.viewer.id(),
        vec![read("ProviderSetupGet", "provider", s.provider)],
    )
    .await;
    let outsider = key(&s.world, s.company.stranger.id(), vec![s.root(true)]).await;
    for token in [
        &wrong_target.secret,
        &wrong_op.secret,
        &read_only.secret,
        &outsider.secret,
    ] {
        denied(setup(&s.world, token, &query).await);
    }
    let wrong_home = places::make_department(
        &s.world,
        &s.company.boss,
        s.company.acme,
        "Other home",
        "other-home",
    )
    .await
    .base
    .id;
    let reply = setup(
        &s.world,
        &actual.secret,
        &ProviderSetupGetQuery {
            org_id: wrong_home,
            ..query
        },
    )
    .await;
    assert_eq!(reply.problem().code, ErrorCode::RequestInvalid);
    denied(setup(&s.world, &actual.secret, &s.query(false)).await);
}

#[tokio::test]
async fn creation_setup_binds_provider_manage_at_the_actual_org_and_preserves_member_policy() {
    let s = Scene::new().await;
    let query = s.query(false);
    let ordinary = shown(setup(&s.world, &s.company.viewer.token, &query).await);
    let actual = key(&s.world, s.company.viewer.id(), vec![s.root(false)]).await;
    let got = shown(setup(&s.world, &actual.secret, &query).await);
    assert_eq!(got["presets"], ordinary["presets"]);
    assert_eq!(got["http_allowed"], ordinary["http_allowed"]);
    let legacy = legacy(&s.world, s.company.viewer.id()).await;
    assert_eq!(
        shown(setup(&s.world, &legacy.secret, &query).await),
        ordinary
    );
    let project = places::make_project(&s.world, &s.company.boss, s.company.acme, "Wrong kind")
        .await
        .base
        .id;
    for rule in [
        root("project", s.company.acme),
        root("org", project),
        root("provider", s.provider),
        json!({"target":{"kind":"org","id":s.company.acme,"reach":"self"},"resources":["provider"],"allow":[{"operation":"ProviderCreate","actions":["manage"]}]}),
    ] {
        let token = key(&s.world, s.company.viewer.id(), vec![rule]).await;
        denied(setup(&s.world, &token.secret, &query).await);
    }
    let outside = key(&s.world, s.company.stranger.id(), vec![s.root(false)]).await;
    denied(setup(&s.world, &outside.secret, &query).await);
    let wrong_kind = setup(
        &s.world,
        &s.company.boss.token,
        &ProviderSetupGetQuery {
            org_id: project,
            provider_id: None,
        },
    )
    .await;
    assert_eq!(wrong_kind.problem().code, ErrorCode::NotFound);
    s.world
        .set_at(
            Target::Node(s.company.acme),
            "resources.created_by_members",
            json!(["host"]),
        )
        .await;
    denied(setup(&s.world, &actual.secret, &query).await);
    denied(setup(&s.world, &s.company.viewer.token, &query).await);
}

#[tokio::test]
async fn root_only_setup_keeps_public_schema_home_values_and_catalogue_without_private_profile_suppression()
 {
    let s = Scene::new().await;
    let kind = "catalogue-fixture";
    let private = agent(
        &s,
        "Private usable profile sentinel",
        kind,
        native_descriptor(kind),
    )
    .await;
    resources::name_person(
        &s.world,
        "agent",
        private,
        "agent.use",
        s.company.viewer.id(),
    )
    .await;
    let mut conn = s.world.conn().await;
    let public = access_fixtures::insert_resource(
        &mut conn,
        "agent",
        None,
        None,
        "Public catalogue fixture",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE agents SET harness_kind=$2,descriptor='{}' WHERE id=$1")
        .bind(public)
        .bind(kind)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let offered_machine = machine(
        &s,
        &s.company.viewer,
        "Ordinary root-only machine",
        "root-only-machine-plan",
    )
    .await;
    let offered_template = template(
        &s,
        "Ordinary root-only template",
        "root-only-template-plan",
        "provider",
        true,
    )
    .await;
    resources::name_person(
        &s.world,
        "template",
        offered_template,
        "template.use",
        s.company.viewer.id(),
    )
    .await;
    s.world.set_at(Target::Node(s.company.acme),"providers.presets",json!({"Home preset":{"upstreams":[{"protocol":"openai-chat","base_url":"https://home-preset.example/v1","options":{}}],"note":"Actual home setup value"}})).await;
    s.world
        .set_at(
            Target::Node(s.company.acme),
            "providers.allow_http_upstreams",
            json!(true),
        )
        .await;
    let ordinary = shown(setup(&s.world, &s.company.viewer.token, &s.query(false)).await);
    assert!(agent_present(&ordinary, private));
    assert!(machine_present(&ordinary, offered_machine));
    assert!(contains(
        &ordinary["runtime_templates"],
        "id",
        offered_template
    ));
    assert!(source(&ordinary, "root-only-machine-plan").is_some());
    assert!(source(&ordinary, "root-only-template-plan").is_some());
    let token = key(&s.world, s.company.viewer.id(), vec![s.root(false)]).await;
    let page = shown(setup(&s.world, &token.secret, &s.query(false)).await);
    assert_eq!(page["presets"], ordinary["presets"]);
    assert_eq!(page["http_allowed"], true);
    assert_eq!(page["api_arrangements"], ordinary["api_arrangements"]);
    let names: Vec<_> = page["protocols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "openai-responses",
            "openai-chat",
            "anthropic-messages",
            "openai-embeddings"
        ]
    );
    assert!(
        page["protocols"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["agents"].as_array().unwrap().is_empty())
    );
    for name in ["adapter", "endpoints"] {
        assert!(source(&page, name).is_some());
    }
    assert!(!agent_present(&page, private));
    for field in [
        "subscriptions",
        "subscriptions_unreported",
        "subscriptions_withheld",
        "subscriptions_unshared",
        "machines",
        "runtime_templates",
        "key_machines",
    ] {
        assert!(
            page[field].as_array().unwrap().is_empty(),
            "a root-only key cannot inherit an external source offer through {field}"
        );
    }
    assert!(
        page["subscriptions_unavailable"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["agent"].is_null())
    );
    assert!(
        page["sources"]
            .as_array()
            .unwrap()
            .iter()
            .all(|source| source["by"] == "builtin")
    );
    let catalogue = page["subscriptions_unavailable"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["harness"]["kind"] == kind)
        .expect("a scoped-out private profile does not suppress a public catalogue entry");
    assert!(catalogue["agent"].is_null());
    assert_eq!(catalogue["reason"], "profile_needed");
    assert_eq!(catalogue["harness"]["name"], "Public catalogue fixture");
    assert!(!page.to_string().contains("Private usable profile sentinel"));
}

#[tokio::test]
async fn actual_agent_read_keeps_complete_use_schema_and_intrinsic_machine_without_management_scope()
 {
    let s = Scene::new().await;
    let selected = agent(
        &s,
        "Usable native profile",
        "fake",
        native_descriptor("fake"),
    )
    .await;
    let private = agent(
        &s,
        "Unusable private profile",
        "private-fixture",
        native_descriptor("private-fixture"),
    )
    .await;
    let disabled = agent(
        &s,
        "Disabled usable profile",
        "disabled-fixture",
        native_descriptor("disabled-fixture"),
    )
    .await;
    let unknown = agent(
        &s,
        "Unreported usable profile",
        "unknown-fixture",
        json!({}),
    )
    .await;
    let linked = machine(
        &s,
        &s.company.boss,
        "Intrinsic profile machine",
        "intrinsic-machine-source",
    )
    .await;
    sqlx::query("UPDATE agents SET machine_id=$2 WHERE id=$1")
        .bind(selected)
        .bind(linked)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE agents SET disabled=true WHERE id=$1")
        .bind(disabled)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    for id in [selected, disabled, unknown] {
        resources::name_person(&s.world, "agent", id, "agent.use", s.company.viewer.id()).await;
    }
    let ordinary = shown(setup(&s.world, &s.company.viewer.token, &s.query(false)).await);
    assert!(agent_present(&ordinary, selected));
    assert!(!agent_present(&ordinary, private) && !agent_present(&ordinary, disabled));
    let wrong = key(
        &s.world,
        s.company.viewer.id(),
        vec![s.root(false), read("ProviderGet", "agent", selected)],
    )
    .await;
    assert!(!agent_present(
        &shown(setup(&s.world, &wrong.secret, &s.query(false)).await),
        selected
    ));
    let token = key(
        &s.world,
        s.company.viewer.id(),
        vec![
            s.root(false),
            read("ProviderSetupGet", "agent", selected),
            read("ProviderSetupGet", "agent", private),
            read("ProviderSetupGet", "agent", disabled),
            read("ProviderSetupGet", "agent", unknown),
        ],
    )
    .await;
    let page = shown(setup(&s.world, &token.secret, &s.query(false)).await);
    let shown_profile = page["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["agent"]["id"] == json!(selected))
        .unwrap();
    let expected = ordinary["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["agent"]["id"] == json!(selected))
        .unwrap();
    assert_eq!(shown_profile, expected);
    assert_eq!(shown_profile["agent"]["machine"]["id"], json!(linked));
    assert_eq!(
        shown_profile["options_schema"],
        native_descriptor("fake")["native_provider"]["options_schema"]
    );
    assert_eq!(shown_profile["methods"][0]["id"], "token");
    assert!(!agent_present(&page, private) && !agent_present(&page, disabled));
    assert!(contains(&page["subscriptions_unreported"], "id", unknown));
    assert!(
        page["subscriptions_unavailable"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |entry| entry["agent"]["id"] == json!(unknown) && entry["reason"] == "not_reported"
            )
    );
    assert!(!page.to_string().contains(PRIVATE_PROFILE));
    s.world
        .set_at(
            Target::Node(s.company.acme),
            "providers.subscription_harnesses",
            json!(["codex"]),
        )
        .await;
    let page = shown(setup(&s.world, &token.secret, &s.query(false)).await);
    assert!(!agent_present(&page, selected));
    assert!(
        page["subscriptions_withheld"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["harness"]["kind"] == "fake")
    );
    let no_source = key(&s.world, s.company.viewer.id(), vec![s.root(false)]).await;
    assert!(
        shown(setup(&s.world, &no_source.secret, &s.query(false)).await)["subscriptions_withheld"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn machine_and_host_use_offer_only_same_operation_machine_capabilities_and_programs() {
    let s = Scene::new().await;
    let mine = machine(&s, &s.company.viewer, "Usable physical machine", "my-plan").await;
    let foreign = machine(
        &s,
        &s.company.admin,
        "Private physical machine",
        "their-plan",
    )
    .await;
    let host = resource(
        &s.world,
        "host",
        &s.company.boss,
        s.company.acme,
        "Usable host facade",
    )
    .await;
    resources::name_person(&s.world, "host", host, "host.use", s.company.viewer.id()).await;
    let hosted: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&s.world.db.pool)
        .await
        .unwrap();
    let ordinary = shown(setup(&s.world, &s.company.viewer.token, &s.query(false)).await);
    assert!(machine_present(&ordinary, mine));
    assert!(machine_present(&ordinary, hosted));
    assert!(!machine_present(&ordinary, foreign));
    assert!(source(&ordinary, "my-plan").is_some());
    let no_source = key(
        &s.world,
        s.company.viewer.id(),
        vec![s.root(false), read("ProviderGet", "machine", mine)],
    )
    .await;
    let hidden = shown(setup(&s.world, &no_source.secret, &s.query(false)).await);
    assert!(!machine_present(&hidden, mine) && source(&hidden, "my-plan").is_none());
    // A host-backed physical machine has no separate Machine permission page. Its
    // ordinary HostUse facade is the actual target, while the facet remains MachineRead.
    let physical_only = key(
        &s.world,
        s.company.viewer.id(),
        vec![
            s.root(false),
            read("ProviderSetupGet", "machine", mine),
            read("ProviderSetupGet", "machine", hosted),
        ],
    )
    .await;
    let unsupported = shown(setup(&s.world, &physical_only.secret, &s.query(false)).await);
    assert!(machine_present(&unsupported, mine));
    assert!(!machine_present(&unsupported, hosted));
    let token = key(
        &s.world,
        s.company.viewer.id(),
        vec![
            s.root(false),
            read("ProviderSetupGet", "machine", mine),
            json!({"target":{"kind":"host","id":host,"reach":"self"},"resources":["machine"],"allow":[{"operation":"ProviderSetupGet","actions":["read"]}]}),
            read("ProviderSetupGet", "machine", foreign),
        ],
    )
    .await;
    let page = shown(setup(&s.world, &token.secret, &s.query(false)).await);
    for id in [mine, hosted] {
        assert!(machine_present(&page, id));
        let actual = page["machines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["machine"]["id"] == json!(id))
            .unwrap();
        let expected = ordinary["machines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["machine"]["id"] == json!(id))
            .unwrap();
        assert_eq!(actual, expected);
    }
    assert!(!machine_present(&page, foreign) && source(&page, "their-plan").is_none());
    assert_eq!(source(&page, "my-plan"), source(&ordinary, "my-plan"));
    assert!(contains(&page["key_machines"], "id", mine));
    let home_scope = key(
        &s.world,
        s.company.viewer.id(),
        vec![
            s.root(false),
            json!({"target":{"kind":"org","id":s.company.acme,"reach":"subtree"},"resources":["machine"],"allow":[{"operation":"ProviderSetupGet","actions":["read"]}]}),
        ],
    )
    .await;
    let in_home = shown(setup(&s.world, &home_scope.secret, &s.query(false)).await);
    assert!(machine_present(&in_home, hosted));
    assert!(!machine_present(&in_home, mine));
}

#[tokio::test]
async fn template_use_read_keeps_declared_commands_but_default_reference_needs_no_template_scope() {
    let s = Scene::new().await;
    let selected = template(
        &s,
        "Selected runtime template",
        "selected-plan",
        "provider",
        true,
    )
    .await;
    let disabled = template(&s, "Disabled template", "disabled-plan", "provider", false).await;
    let environment = template(
        &s,
        "Environment template",
        "environment-plan",
        "environment",
        true,
    )
    .await;
    let profile = agent(
        &s,
        "Native for default reference",
        "fake",
        native_descriptor("fake"),
    )
    .await;
    for id in [selected, disabled, environment] {
        resources::name_person(
            &s.world,
            "template",
            id,
            "template.use",
            s.company.viewer.id(),
        )
        .await;
    }
    resources::name_person(
        &s.world,
        "agent",
        profile,
        "agent.use",
        s.company.viewer.id(),
    )
    .await;
    s.world
        .set_at(
            Target::Node(s.company.acme),
            "runtime.template_id",
            json!(selected),
        )
        .await;
    let ordinary = shown(setup(&s.world, &s.company.viewer.token, &s.query(false)).await);
    assert!(contains(&ordinary["runtime_templates"], "id", selected));
    assert!(source(&ordinary, "selected-plan").is_some());
    let intrinsic = key(
        &s.world,
        s.company.viewer.id(),
        vec![s.root(false), read("ProviderSetupGet", "agent", profile)],
    )
    .await;
    let hidden = shown(setup(&s.world, &intrinsic.secret, &s.query(false)).await);
    let subscription = hidden["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["agent"]["id"] == json!(profile))
        .unwrap();
    assert_eq!(subscription["template"]["id"], json!(selected));
    assert_eq!(
        subscription["template"]["name"],
        "Selected runtime template"
    );
    assert!(
        !contains(&hidden["runtime_templates"], "id", selected)
            && source(&hidden, "selected-plan").is_none()
    );
    let token = key(
        &s.world,
        s.company.viewer.id(),
        vec![
            s.root(false),
            read("ProviderSetupGet", "template", selected),
            read("ProviderSetupGet", "template", disabled),
            read("ProviderSetupGet", "template", environment),
            read("ProviderSetupGet", "agent", profile),
        ],
    )
    .await;
    let page = shown(setup(&s.world, &token.secret, &s.query(false)).await);
    assert!(contains(&page["runtime_templates"], "id", selected));
    assert!(
        !contains(&page["runtime_templates"], "id", disabled)
            && !contains(&page["runtime_templates"], "id", environment)
    );
    assert_eq!(
        source(&page, "selected-plan").unwrap()["command"],
        json!(["/opt/useful-plan-reader", "--json"])
    );
    assert_eq!(
        source(&page, "selected-plan"),
        source(&ordinary, "selected-plan")
    );
    assert!(!page.to_string().contains(PRIVATE_TEMPLATE));
}

#[tokio::test]
async fn existing_manager_keeps_exact_keeper_and_runtime_programs_without_new_resource_use() {
    let s = Scene::new().await;
    resources::name_person(
        &s.world,
        "provider",
        s.provider,
        "provider.manage",
        s.company.viewer.id(),
    )
    .await;
    s.world
        .set_at(
            Target::Node(s.company.acme),
            "resources.created_by_members",
            json!(["host"]),
        )
        .await;
    let keeper = machine(
        &s,
        &s.company.boss,
        "Current unrelated-owner keeper",
        "current-keeper-plan",
    )
    .await;
    let replacement = machine(
        &s,
        &s.company.admin,
        "Replacement keeper",
        "replacement-plan",
    )
    .await;
    let own = machine(
        &s,
        &s.company.viewer,
        "Unrelated own keeper",
        "own-unrelated-plan",
    )
    .await;
    sqlx::query("UPDATE providers SET key_machine_id=$2 WHERE id=$1")
        .bind(s.provider)
        .bind(keeper)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let query = s.query(true);
    let ordinary = shown(setup(&s.world, &s.company.viewer.token, &query).await);
    assert!(
        source(&ordinary, "current-keeper-plan").is_some()
            && source(&ordinary, "own-unrelated-plan").is_none()
    );
    assert_eq!(
        ordinary["key_machines"].as_array().unwrap()[0]["id"],
        json!(keeper)
    );
    let root_only = key(&s.world, s.company.viewer.id(), vec![s.root(true)]).await;
    let hidden = shown(setup(&s.world, &root_only.secret, &query).await);
    assert_eq!(hidden["key_machines"], ordinary["key_machines"]);
    assert!(source(&hidden, "current-keeper-plan").is_none());
    let token = key(
        &s.world,
        s.company.viewer.id(),
        vec![
            s.root(true),
            read("ProviderSetupGet", "machine", keeper),
            read("ProviderSetupGet", "machine", own),
        ],
    )
    .await;
    let page = shown(setup(&s.world, &token.secret, &query).await);
    assert_eq!(
        source(&page, "current-keeper-plan"),
        source(&ordinary, "current-keeper-plan")
    );
    assert!(source(&page, "own-unrelated-plan").is_none());
    sqlx::query("UPDATE providers SET key_machine_id=$2 WHERE id=$1")
        .bind(s.provider)
        .bind(replacement)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let changed = shown(setup(&s.world, &token.secret, &query).await);
    assert_eq!(changed["key_machines"][0]["id"], json!(replacement));
    assert!(
        source(&changed, "current-keeper-plan").is_none()
            && source(&changed, "replacement-plan").is_none()
    );
    let current = key(
        &s.world,
        s.company.viewer.id(),
        vec![
            s.root(true),
            read("ProviderSetupGet", "machine", replacement),
        ],
    )
    .await;
    assert!(
        source(
            &shown(setup(&s.world, &current.secret, &query).await),
            "replacement-plan"
        )
        .is_some()
    );
    let selected = template(
        &s,
        "Exact runtime without TemplateUse",
        "exact-runtime-plan",
        "provider",
        true,
    )
    .await;
    let other = template(
        &s,
        "Unrelated usable runtime",
        "other-runtime-plan",
        "provider",
        true,
    )
    .await;
    resources::name_person(
        &s.world,
        "template",
        other,
        "template.use",
        s.company.viewer.id(),
    )
    .await;
    sqlx::query("UPDATE providers SET key_machine_id=NULL WHERE id=$1")
        .bind(s.provider)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    desired_template(&s, selected).await;
    let ordinary = shown(setup(&s.world, &s.company.viewer.token, &query).await);
    assert!(
        source(&ordinary, "exact-runtime-plan").is_some()
            && source(&ordinary, "other-runtime-plan").is_none()
    );
    let token = key(
        &s.world,
        s.company.viewer.id(),
        vec![
            s.root(true),
            read("ProviderSetupGet", "template", selected),
            read("ProviderSetupGet", "template", other),
        ],
    )
    .await;
    let page = shown(setup(&s.world, &token.secret, &query).await);
    assert_eq!(
        source(&page, "exact-runtime-plan"),
        source(&ordinary, "exact-runtime-plan")
    );
    assert!(source(&page, "other-runtime-plan").is_none());
    assert!(
        machine_present(&ordinary, own),
        "ordinary unrelated machine capability choice is separate from exact keeper-program choices"
    );

    // The selected declaration remains readable through its retained source home. This
    // deliberately preserves the enabled template row; deleting its home through the
    // neutral hierarchy helper must not restore that home or impose new TemplateUse.
    let source_home = places::make_org(
        &s.world,
        &s.company.boss,
        "Retained template home",
        "retained-template-home",
    )
    .await
    .base
    .id;
    sqlx::query("UPDATE env_templates SET home_id=$2,org_id=$2 WHERE id=$1")
        .bind(selected)
        .bind(source_home)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let before = shown(setup(&s.world, &s.company.viewer.token, &query).await);
    assert!(source(&before, "exact-runtime-plan").is_some());
    orch_db::queries::hierarchy::delete_subtree(
        &mut *s.world.conn().await,
        source_home,
        s.company.boss.id(),
        s.world.now(),
    )
    .await
    .unwrap();
    let state = || async {
        sqlx::query_as::<_, (Option<chrono::DateTime<chrono::Utc>>, Option<chrono::DateTime<chrono::Utc>>, Id)>(
            "SELECT (SELECT deleted_at FROM nodes WHERE id=$1), t.deleted_at, (SELECT home_id FROM providers WHERE id=$3) FROM env_templates t WHERE t.id=$2")
            .bind(source_home).bind(selected).bind(s.provider).fetch_one(&s.world.db.pool).await.unwrap()
    };
    let retained = state().await;
    assert!(retained.0.is_some() && retained.1.is_none());
    assert_eq!(retained.2, s.company.acme);
    let ordinary = shown(setup(&s.world, &s.company.viewer.token, &query).await);
    assert_eq!(
        source(&ordinary, "exact-runtime-plan"),
        source(&before, "exact-runtime-plan")
    );
    let legacy = legacy(&s.world, s.company.viewer.id()).await;
    assert_eq!(
        shown(setup(&s.world, &legacy.secret, &query).await),
        ordinary
    );
    let exact = key(
        &s.world,
        s.company.viewer.id(),
        vec![s.root(true), read("ProviderSetupGet", "template", selected)],
    )
    .await;
    assert_eq!(
        source(
            &shown(setup(&s.world, &exact.secret, &query).await),
            "exact-runtime-plan"
        ),
        source(&ordinary, "exact-runtime-plan")
    );
    for (home, expected) in [(source_home, true), (s.company.acme, false)] {
        let scoped = key(&s.world, s.company.viewer.id(), vec![s.root(true), json!({"target":{"kind":"org","id":home,"reach":"subtree"},"resources":["template"],"allow":[{"operation":"ProviderSetupGet","actions":["read"]}]})]).await;
        let page = shown(setup(&s.world, &scoped.secret, &query).await);
        assert_eq!(
            source(&page, "exact-runtime-plan").is_some(),
            expected,
            "scope uses the template's current retained home"
        );
    }
    assert_eq!(
        state().await,
        retained,
        "setup never restores its retained source home or changes provider placement"
    );
}

#[tokio::test]
async fn policy_source_names_are_optional_same_operation_reads_while_values_and_public_identity_remain()
 {
    let s = Scene::new().await;
    let profile = agent(
        &s,
        "Allowed profile schema",
        "fake",
        native_descriptor("fake"),
    )
    .await;
    resources::name_person(
        &s.world,
        "agent",
        profile,
        "agent.use",
        s.company.viewer.id(),
    )
    .await;
    s.world
        .set_at(
            Target::Node(s.company.acme),
            "providers.subscription_harnesses",
            json!(["codex"]),
        )
        .await;
    let ordinary = shown(setup(&s.world, &s.company.viewer.token, &s.query(false)).await);
    assert_eq!(
        ordinary["subscriptions_withheld"][0]["scope"]["name"],
        "Acme"
    );
    let mut rules = vec![
        s.root(false),
        read("ProviderSetupGet", "agent", profile),
        read("ProviderGet", "org", s.company.acme),
    ];
    let narrow = key(&s.world, s.company.viewer.id(), rules.clone()).await;
    let hidden = shown(setup(&s.world, &narrow.secret, &s.query(false)).await);
    assert_eq!(
        hidden["subscriptions_withheld"][0]["scope"]["id"],
        json!(s.company.acme)
    );
    assert!(hidden["subscriptions_withheld"][0]["scope"]["name"].is_null());
    assert_eq!(
        hidden["subscriptions_withheld"][0]["scope"]["redacted"],
        true
    );
    assert_eq!(hidden["presets"], ordinary["presets"]);
    assert_eq!(hidden["http_allowed"], ordinary["http_allowed"]);
    rules.push(read("ProviderSetupGet", "org", s.company.acme));
    let full = key(&s.world, s.company.viewer.id(), rules).await;
    assert_eq!(
        shown(setup(&s.world, &full.secret, &s.query(false)).await)["subscriptions_withheld"],
        ordinary["subscriptions_withheld"]
    );
}

async fn checkpoint(narrow: bool) {
    let s = Scene::new().await;
    let token = key(&s.world, s.company.viewer.id(), vec![s.root(false)]).await;
    let mut blocker = s.world.db.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_person(&mut blocker, s.company.viewer.id())
        .await
        .unwrap()
        .unwrap();
    sqlx::query("SELECT id FROM api_tokens WHERE id=$1 AND user_id=$2 FOR UPDATE")
        .bind(token.id)
        .bind(token.user)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
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
        .uri(format!("/provider-setup?org_id={}", s.company.acme))
        .header("authorization", format!("Bearer {}", token.secret))
        .body(Body::empty())
        .unwrap();
    let action = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    let waiting=tokio::time::timeout(LOCK_WAIT,async{loop{if action.is_finished(){break false;}let waits:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))").bind(pid).fetch_one(&s.world.db.pool).await.unwrap();if waits{break true;}tokio::task::yield_now().await;}}).await.expect("setup waits on its current identity or completes");
    if narrow {
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
            .bind(token.id)
            .bind(serde_json::to_value(KeyScope::default()).unwrap())
            .execute(&mut *blocker)
            .await
            .unwrap();
    }
    blocker.commit().await.unwrap();
    let response = tokio::time::timeout(LOCK_WAIT, action)
        .await
        .expect("released setup finishes")
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), MAX_JSON_BYTES)
        .await
        .unwrap();
    assert_eq!(
        status,
        if narrow {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::OK
        },
        "actual lock observed={waiting}; {}",
        String::from_utf8_lossy(&bytes)
    );
    assert!(
        waiting,
        "the setup read checkpoint must actually wait on current identity"
    );
    if narrow {
        let problem: Problem = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(problem.code, ErrorCode::AuthzDenied);
        assert!(!String::from_utf8_lossy(&bytes).contains("protocols"));
    }
}

#[tokio::test]
async fn setup_restores_current_key_rows_and_ordinary_rights_on_its_actual_snapshot() {
    let s = Scene::new().await;
    for revoke in [false, true] {
        let token = key(&s.world, s.company.viewer.id(), vec![s.root(false)]).await;
        shown(setup(&s.world, &token.secret, &s.query(false)).await);
        if revoke {
            identity::revoke_api_token(
                &mut *s.world.conn().await,
                token.id,
                token.user,
                s.world.now(),
            )
            .await
            .unwrap();
        } else {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(token.id)
                .bind(serde_json::to_value(KeyScope::default()).unwrap())
                .execute(&s.world.db.pool)
                .await
                .unwrap();
        }
        let after = setup(&s.world, &token.secret, &s.query(false)).await;
        assert_eq!(
            after.status,
            if revoke {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::FORBIDDEN
            }
        );
        assert!(!String::from_utf8_lossy(&after.raw).contains("protocols"));
    }
    checkpoint(false).await;
    checkpoint(true).await;
    let entry = resources::name_person(
        &s.world,
        "provider",
        s.provider,
        "provider.manage",
        s.company.viewer.id(),
    )
    .await;
    let manager = key(&s.world, s.company.viewer.id(), vec![s.root(true)]).await;
    shown(setup(&s.world, &manager.secret, &s.query(true)).await);
    let mut tx = s.world.db.pool.begin().await.unwrap();
    assert!(
        orch_db::queries::access::remove_entry(&mut tx, entry, s.company.boss.id(), s.world.now(),)
            .await
            .unwrap()
    );
    orch_db::queries::access::bump_epoch(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    denied(setup(&s.world, &manager.secret, &s.query(true)).await);
    denied(setup(&s.world, &s.company.viewer.token, &s.query(true)).await);
}

#[tokio::test]
async fn setup_emits_useful_schema_and_program_declarations_without_native_effects_or_private_configuration()
 {
    let s = Scene::new().await;
    let profile = agent(
        &s,
        "Declared usable profile",
        "fake",
        native_descriptor("fake"),
    )
    .await;
    let machine = machine(
        &s,
        &s.company.boss,
        "Declared keeper",
        "declared-keeper-plan",
    )
    .await;
    let template = template(
        &s,
        "Declared template",
        "declared-template-plan",
        "provider",
        true,
    )
    .await;
    desired_template(&s, template).await;
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([7; 32]),
    );
    orch_api_core::routes::resources::sealing::put_provider_secret(
        &mut *s.world.conn().await,
        s.provider,
        "setup-test-secret",
        PRIVATE_CREDENTIAL,
        s.company.boss.id(),
        s.world.now(),
    )
    .await
    .unwrap();
    let before = facts(&s.world).await;
    for existing in [false, true] {
        let token = key(
            &s.world,
            s.company.boss.id(),
            vec![
                s.root(existing),
                read("ProviderSetupGet", "agent", profile),
                read("ProviderSetupGet", "machine", machine),
                read("ProviderSetupGet", "template", template),
            ],
        )
        .await;
        let page = shown(setup(&s.world, &token.secret, &s.query(existing)).await);
        assert!(agent_present(&page, profile));
        let schema = &page["subscriptions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["agent"]["id"] == json!(profile))
            .unwrap()["options_schema"];
        assert_eq!(
            *schema,
            native_descriptor("fake")["native_provider"]["options_schema"]
        );
        assert_eq!(
            source(&page, "declared-template-plan").unwrap()["command"],
            json!(["/opt/useful-plan-reader", "--json"])
        );
        if !existing {
            assert!(source(&page, "declared-keeper-plan").is_some());
        }
        let raw = page.to_string();
        for sentinel in [PRIVATE_PROFILE, PRIVATE_TEMPLATE, PRIVATE_CREDENTIAL] {
            assert!(
                !raw.contains(sentinel),
                "only declared setup output is public, not {sentinel}"
            );
        }
    }
    assert_eq!(
        facts(&s.world).await,
        before,
        "setup reads issue no native commands/jobs and rotate no secret"
    );
}

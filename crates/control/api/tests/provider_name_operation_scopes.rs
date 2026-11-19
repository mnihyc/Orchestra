//! ProviderGet bounds protected live names without rewriting permissions, identities or counts.
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
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{GroupMemberAdd, OrgGroupCreate, OrgGroupCreateBody};
use orch_api_contract::nodes::{DeletedInspectionQuery, NodeGet};
use orch_api_contract::resources::ProviderGet;
use orch_api_contract::usage::UsageCapCreate;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{identity, resources::access_fixtures};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use support::World;

async fn provider(world: &World, company: &resources::Company) -> Id {
    let mut conn = world.conn().await;
    let id = access_fixtures::insert_resource(
        &mut conn,
        "provider",
        Some(company.boss.id()),
        Some(company.acme),
        "Provider identity stays visible",
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Provider.into(),
            id,
        },
        company.boss.id(),
        NewAccess::OnlyMe,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    id
}

fn read_rule(kind: &str, id: Id, operation: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},
        "allow":[{"operation":operation,"actions":["read"]}]})
}

async fn key(world: &World, user: Id, provider: Id, sources: Vec<Json>) -> String {
    let mut rules = vec![read_rule("provider", provider, "ProviderGet")];
    rules.extend(sources);
    let scope: orch_access_contract::key_scope::KeyScope =
        serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Provider live-name control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: world.now(),
            // Fixed-clock read controls use the existing ordinary one-hour fixture lifetime.
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}

async fn get(world: &World, token: &str, provider: Id) -> RawReply {
    world
        .app
        .call(ProviderGet, &[&provider])
        .token(token)
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

fn redacted(node: &Json, id: Id, kind: &str) {
    assert_eq!(node["id"], json!(id));
    assert_eq!(node["kind"], kind);
    assert!(node["name"].is_null());
    assert_eq!(node["redacted"], true);
}

fn identity_unchanged(value: &Json, ordinary: &Json) {
    for field in ["id", "name", "address", "owner", "owners", "org_id"] {
        assert_eq!(
            value[field], ordinary[field],
            "public provider/owner identity: {field}"
        );
    }
    assert!(!value["address"].as_str().unwrap().is_empty());
    assert!(value["owner"]["username"].as_str().is_some());
}

async fn snapshot(world: &World, id: Id) -> Json {
    sqlx::query_scalar("SELECT jsonb_build_object('provider',to_jsonb(p),
        'rows',(SELECT jsonb_agg(to_jsonb(r) ORDER BY r.permission,r.profile) FROM permission_rows r WHERE r.object_kind='provider' AND r.object_id=p.id),
        'entries',(SELECT jsonb_agg(to_jsonb(e) ORDER BY e.id) FROM permission_entries e WHERE e.object_kind='provider' AND e.object_id=p.id),
        'caps',(SELECT jsonb_agg(to_jsonb(c) ORDER BY c.id) FROM usage_caps c WHERE c.provider_id=p.id))
        FROM providers p WHERE p.id=$1")
        .bind(id).fetch_one(&world.db.pool).await.unwrap()
}

#[tokio::test]
async fn provider_home_names_need_same_operation_node_read_without_hiding_public_provider_identity()
{
    let world = World::new().await;
    let company = resources::company(&world).await;
    let id = provider(&world, &company).await;
    // The browser proves ordinary NodeView separately from the key's source restriction.
    let home = world
        .app
        .call(NodeGet, &[&company.acme])
        .token(&company.boss.token)
        .query(&DeletedInspectionQuery::default())
        .send_bytes()
        .await;
    assert_eq!(home.status, StatusCode::OK);
    let ordinary = shown(get(&world, &company.boss.token, id).await);
    assert_eq!(ordinary["home"]["name"], "Acme");
    assert_eq!(ordinary["home"]["redacted"], false);
    let before = snapshot(&world, id).await;
    let bounded = key(&world, company.boss.id(), id, vec![]).await;
    let hidden = shown(get(&world, &bounded, id).await);
    redacted(&hidden["home"], company.acme, "org");
    identity_unchanged(&hidden, &ordinary);
    let unrelated = key(
        &world,
        company.boss.id(),
        id,
        vec![read_rule("org", company.acme, "NodeGet.read")],
    )
    .await;
    let still_hidden = shown(get(&world, &unrelated, id).await);
    redacted(&still_hidden["home"], company.acme, "org");
    identity_unchanged(&still_hidden, &ordinary);
    let permitted = key(
        &world,
        company.boss.id(),
        id,
        vec![read_rule("org", company.acme, "ProviderGet")],
    )
    .await;
    let named = shown(get(&world, &permitted, id).await);
    assert_eq!(named["home"], ordinary["home"]);
    identity_unchanged(&named, &ordinary);
    assert_eq!(
        hidden["access"]["permissions"],
        named["access"]["permissions"]
    );
    assert_eq!(
        still_hidden["access"]["permissions"],
        named["access"]["permissions"]
    );
    assert_eq!(snapshot(&world, id).await, before);
}

#[tokio::test]
async fn provider_node_source_scope_cannot_replace_the_accounts_ordinary_node_view() {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let id = provider(&world, &company).await;
    resources::name_person(
        &world,
        "provider",
        id,
        "provider.view",
        company.stranger.id(),
    )
    .await;
    let denied_home = world
        .app
        .call(NodeGet, &[&company.acme])
        .token(&company.stranger.token)
        .query(&DeletedInspectionQuery::default())
        .send_bytes()
        .await;
    assert_eq!(denied_home.status, StatusCode::FORBIDDEN);
    let ordinary = shown(get(&world, &company.stranger.token, id).await);
    redacted(&ordinary["home"], company.acme, "org");
    let scoped = key(
        &world,
        company.stranger.id(),
        id,
        vec![read_rule("org", company.acme, "ProviderGet")],
    )
    .await;
    let bounded = shown(get(&world, &scoped, id).await);
    redacted(&bounded["home"], company.acme, "org");
    identity_unchanged(&bounded, &ordinary);
}

fn holder<'a>(provider: &'a Json, kind: &str) -> &'a Json {
    provider["access"]["through"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|through| {
            (through["kind"] == "entry" && through["holder"]["kind"] == kind)
                .then_some(&through["holder"])
        })
        .expect("the actual permission holder remains in the access explanation")
}

#[tokio::test]
async fn provider_access_role_labels_ancestors_and_domain_addresses_obey_name_bounds_without_changing_counts()
 {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let project =
        places::make_project(&world, &company.boss, company.acme, "Protected project").await;
    let topic = places::make_topic(&world, &company.boss, project.base.id, "Protected topic").await;
    let id = provider(&world, &company).await;
    resources::name_on_page(
        &world,
        "provider",
        id,
        "provider.view",
        ("role", Some(topic.base.id), Some("viewer")),
    )
    .await;
    resources::name_person(&world, "provider", id, "provider.view", company.viewer.id()).await;
    let ordinary = shown(get(&world, &company.viewer.token, id).await);
    let ordinary_role = holder(&ordinary, "role");
    let label = ordinary_role["at"]["name"].as_str().unwrap();
    for part in ["Acme", "Protected project", "Protected topic"] {
        assert!(label.contains(part), "{label}");
    }
    assert!(ordinary_role["count"].as_i64().unwrap() > 0);
    let ordinary_user = holder(&ordinary, "user");
    assert!(
        ordinary_user["user"]["address"]
            .as_str()
            .unwrap()
            .contains("viewer")
    );
    let before = snapshot(&world, id).await;
    let bounded = key(&world, company.viewer.id(), id, vec![]).await;
    let hidden = shown(get(&world, &bounded, id).await);
    let hidden_role = holder(&hidden, "role");
    redacted(&hidden_role["at"], topic.base.id, "topic");
    assert!(hidden_role["place"].is_null());
    let hidden_user = holder(&hidden, "user");
    assert_eq!(hidden_user["user"]["id"], json!(company.viewer.id()));
    assert_eq!(hidden_user["user"]["username"], "viewer");
    // The public @username remains; only the protected domain qualification is omitted.
    assert_eq!(hidden_user["user"]["address"], "@viewer");
    let leaf = key(
        &world,
        company.viewer.id(),
        id,
        vec![read_rule("topic", topic.base.id, "ProviderGet")],
    )
    .await;
    let partial = shown(get(&world, &leaf, id).await);
    let leaf_label = holder(&partial, "role")["at"]["name"].as_str().unwrap();
    assert!(leaf_label.contains("Protected topic"));
    assert!(!leaf_label.contains("Protected project") && !leaf_label.contains("Acme"));
    assert_eq!(holder(&partial, "user")["user"]["address"], "@viewer");
    let all = key(
        &world,
        company.viewer.id(),
        id,
        vec![
            read_rule("topic", topic.base.id, "ProviderGet"),
            read_rule("project", project.base.id, "ProviderGet"),
            read_rule("org", company.acme, "ProviderGet"),
        ],
    )
    .await;
    let named = shown(get(&world, &all, id).await);
    assert_eq!(holder(&named, "role"), ordinary_role);
    let named_user = &holder(&named, "user")["user"];
    assert_eq!(named_user["id"], ordinary_user["user"]["id"]);
    assert_eq!(named_user["username"], ordinary_user["user"]["username"]);
    // This account also owns a personal org. Only Acme's domain source was admitted here.
    assert_eq!(named_user["address"], "acme.viewer");
    for value in [&hidden, &partial, &named] {
        assert_eq!(
            value["access"]["permissions"],
            ordinary["access"]["permissions"]
        );
        assert_eq!(
            value["access"]["through"].as_array().unwrap().len(),
            ordinary["access"]["through"].as_array().unwrap().len()
        );
        assert_eq!(holder(value, "role")["role"], ordinary_role["role"]);
        assert_eq!(holder(value, "role")["count"], ordinary_role["count"]);
        identity_unchanged(value, &ordinary);
    }
    assert_eq!(snapshot(&world, id).await, before);
}

struct GroupCase {
    world: World,
    company: resources::Company,
    provider: Id,
    access_group: Id,
    cap_group: Id,
    cap: Id,
}

async fn group_case() -> GroupCase {
    let world = World::new().await;
    let company = resources::company(&world).await;
    let provider = provider(&world, &company).await;
    let mut groups = Vec::new();
    for name in ["Access group sentinel", "Cap group sentinel"] {
        let group = world
            .app
            .call(OrgGroupCreate, &[&company.acme])
            .token(&company.boss.token)
            .body(&OrgGroupCreateBody {
                name: name.into(),
                description: Some(format!("{name} description")),
            })
            .send()
            .await
            .ok()
            .into_full()
            .expect("this ordinary control requires the full native creation response");
        world
            .app
            .call(GroupMemberAdd, &[&group.id, &company.viewer.id()])
            .token(&company.boss.token)
            .send()
            .await
            .ok();
        groups.push(group.id);
    }
    // Distinct nonzero counts expose either invented zero counts or a dropped Group source.
    world
        .app
        .call(GroupMemberAdd, &[&groups[0], &company.admin.id()])
        .token(&company.boss.token)
        .send()
        .await
        .ok();
    resources::name_on_page(
        &world,
        "provider",
        provider,
        "provider.view",
        ("group", Some(groups[0]), None),
    )
    .await;
    let cap = world
        .app
        .call(UsageCapCreate, &[])
        .token(&company.boss.token)
        .raw_body(
            &json!({"provider_id":provider,"subject":{"kind":"group","id":groups[1]},
            "split":"shared","meter":"running","amount":"1"}),
        )
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .id;
    GroupCase {
        world,
        company,
        provider,
        access_group: groups[0],
        cap_group: groups[1],
        cap,
    }
}

fn groups_preserved(value: &Json, case: &GroupCase) {
    let access = &holder(value, "group")["group"];
    assert_eq!(access["id"], json!(case.access_group));
    assert_eq!(access["org_id"], json!(case.company.acme));
    assert_eq!(access["name"], "Access group sentinel");
    assert_eq!(access["description"], "Access group sentinel description");
    assert_eq!(access["member_count"], 2);
    let caps = value["choice_usage"]["caps"].as_array().unwrap();
    assert_eq!(caps.len(), 1, "the one real applicable cap remains");
    let cap = &caps[0]["cap"];
    assert_eq!(cap["id"], json!(case.cap));
    assert_eq!(cap["provider"]["id"], json!(case.provider));
    assert_eq!(cap["subject"]["kind"], "group");
    let group = &cap["subject"]["group"];
    assert_eq!(group["id"], json!(case.cap_group));
    assert_eq!(group["org_id"], json!(case.company.acme));
    assert_eq!(group["name"], "Cap group sentinel");
    assert_eq!(group["description"], "Cap group sentinel description");
    assert_eq!(group["member_count"], 1);
}

async fn missing_group_read(access_allowed: bool, cap_allowed: bool, unrelated: bool) {
    let case = group_case().await;
    let ordinary = shown(get(&case.world, &case.company.viewer.token, case.provider).await);
    groups_preserved(&ordinary, &case);
    let before = snapshot(&case.world, case.provider).await;
    let mut sources = vec![read_rule("org", case.company.acme, "ProviderGet")];
    for (group, allowed) in [
        (case.access_group, access_allowed),
        (case.cap_group, cap_allowed),
    ] {
        if allowed {
            sources.push(read_rule("group", group, "ProviderGet"));
        } else if unrelated {
            sources.push(read_rule("group", group, "GroupMemberList"));
        }
    }
    let scoped = key(
        &case.world,
        case.company.viewer.id(),
        case.provider,
        sources,
    )
    .await;
    let refused = get(&case.world, &scoped, case.provider).await;
    assert_eq!(
        refused.status,
        StatusCode::FORBIDDEN,
        "full Group DTOs require their actual source Read"
    );
    assert_eq!(refused.problem().code, ErrorCode::AuthzDenied);
    let body = String::from_utf8_lossy(&refused.raw);
    assert!(!body.contains("Access group sentinel") && !body.contains("Cap group sentinel"));
    assert_eq!(snapshot(&case.world, case.provider).await, before);
}

#[tokio::test]
async fn provider_access_group_requires_its_actual_same_operation_group_read() {
    missing_group_read(false, true, false).await;
}

#[tokio::test]
async fn provider_applicable_cap_group_requires_its_actual_same_operation_group_read() {
    missing_group_read(true, false, false).await;
}

#[tokio::test]
async fn another_group_read_operation_cannot_lend_provider_group_disclosure() {
    missing_group_read(false, false, true).await;
}

#[tokio::test]
async fn provider_same_operation_group_reads_preserve_full_groups_applicable_caps_and_counts() {
    let case = group_case().await;
    let ordinary = shown(get(&case.world, &case.company.viewer.token, case.provider).await);
    groups_preserved(&ordinary, &case);
    let before = snapshot(&case.world, case.provider).await;
    let scoped = key(
        &case.world,
        case.company.viewer.id(),
        case.provider,
        vec![
            read_rule("org", case.company.acme, "ProviderGet"),
            read_rule("group", case.access_group, "ProviderGet"),
            read_rule("group", case.cap_group, "ProviderGet"),
        ],
    )
    .await;
    let permitted = shown(get(&case.world, &scoped, case.provider).await);
    groups_preserved(&permitted, &case);
    assert_eq!(permitted["access"], ordinary["access"]);
    assert_eq!(
        permitted["choice_usage"]["caps"],
        ordinary["choice_usage"]["caps"]
    );
    identity_unchanged(&permitted, &ordinary);
    assert_eq!(snapshot(&case.world, case.provider).await, before);
}

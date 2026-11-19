//! Template collections retain ordinary windows while bounding actual rows and live suppliers.
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
use orch_api_contract::resources::{TemplateList, TemplateListQuery, UnitKind};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::identity::{self, NewApiToken};
use orch_db::queries::resources::access_fixtures as fixtures;
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
    ids: Vec<Id>,
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
impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        let company = resources::company(&world).await;
        let mut ids = Vec::new();
        for name in [
            "First listed template",
            "Second listed template",
            "Third listed template",
        ] {
            ids.push(
                template(
                    &world,
                    &company.boss,
                    Some(company.acme),
                    name,
                    "environment",
                )
                .await,
            );
        }
        ids.sort_unstable();
        Self {
            world,
            company,
            ids,
        }
    }
}
fn rule(operation: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]})
}
fn subtree(operation: &str, org: Id) -> Json {
    json!({"target":{"kind":"org","id":org,"reach":"subtree"},"allow":[{"operation":operation,"actions":["read"]}]})
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
            name: "Template list control",
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
async fn list(world: &World, token: &str, query: &TemplateListQuery) -> RawReply {
    world
        .app
        .call(TemplateList, &[])
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
        .expect("the admitted template remains")
}
fn denied(reply: RawReply) {
    assert_eq!(
        reply.status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(
        reply.problem().code,
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
}
fn holder<'a>(template: &'a Json, kind: &str) -> &'a Json {
    template["access"]["through"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|through| {
            (through["kind"] == "entry" && through["holder"]["kind"] == kind)
                .then_some(&through["holder"])
        })
        .expect("the actual holder remains")
}

#[tokio::test]
async fn exact_template_reads_select_only_real_results_and_do_not_borrow_other_operations() {
    let s = Scene::new().await;
    let ordinary = shown(list(&s.world, &s.company.boss.token, &Default::default()).await);
    assert!(s.ids.iter().all(|id| ids(&ordinary).contains(id)));
    let selected = s.ids[1];
    let token = key(
        &s.world,
        s.company.boss.id(),
        vec![
            rule("TemplateList", "template", selected),
            rule("TemplateList", "org", s.company.acme),
        ],
    )
    .await;
    let page = shown(list(&s.world, &token.secret, &Default::default()).await);
    assert_eq!(ids(&page), [selected]);
    assert_eq!(row(&page, selected), row(&ordinary, selected));
    let mut wrong_resource = rule("TemplateList", "template", selected);
    wrong_resource["resources"] = json!(["provider"]);
    let mut wrong_action = rule("TemplateList", "template", selected);
    wrong_action["allow"][0]["actions"] = json!(["write"]);
    for rules in [
        Vec::new(),
        vec![rule("TemplateGet", "template", selected)],
        vec![wrong_resource],
        vec![wrong_action],
    ] {
        let token = key(&s.world, s.company.boss.id(), rules).await;
        denied(list(&s.world, &token.secret, &Default::default()).await);
    }
    let stranger = key(
        &s.world,
        s.company.stranger.id(),
        vec![rule("TemplateList", "template", selected)],
    )
    .await;
    assert!(
        ids(&shown(
            list(&s.world, &stranger.secret, &Default::default()).await
        ))
        .is_empty(),
        "scope cannot manufacture ordinary visibility"
    );
}

#[tokio::test]
async fn ordinary_org_kind_and_system_inclusion_keep_equal_authority_browser_and_legacy_parity() {
    let s = Scene::new().await;
    let system = template(
        &s.world,
        &s.company.boss,
        None,
        "System template",
        "provider",
    )
    .await;
    resources::name_person(
        &s.world,
        "template",
        system,
        "template.use",
        s.company.boss.id(),
    )
    .await;
    let runtime = s.ids[0];
    sqlx::query("UPDATE env_templates SET kind='provider' WHERE id=$1")
        .bind(runtime)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let legacy = issue(&s.world, s.company.boss.id(), None).await;
    for kind in [None, Some(UnitKind::Environment), Some(UnitKind::Provider)] {
        let query = TemplateListQuery {
            org_id: Some(s.company.acme),
            kind,
        };
        let ordinary = shown(list(&s.world, &s.company.boss.token, &query).await);
        assert_eq!(
            shown(list(&s.world, &legacy.secret, &query).await),
            ordinary
        );
        if query.kind != Some(UnitKind::Environment) {
            assert!(ids(&ordinary).contains(&system));
        }
        let mut rules = vec![subtree("TemplateList", s.company.acme)];
        rules.push(rule("TemplateList", "template", system));
        let token = key(&s.world, s.company.boss.id(), rules).await;
        assert_eq!(shown(list(&s.world, &token.secret, &query).await), ordinary);
    }
}

#[tokio::test]
async fn scoped_template_selection_crosses_the_ordinary_first_window_without_changing_that_window()
{
    let world = World::new().await;
    let company = resources::company(&world).await;
    let mut made = Vec::new();
    // Exactly the established LIST_LIMIT plus one: this is a page-boundary control, not scale brute force.
    for index in 0..501 {
        let name = format!("Window template {index}");
        let id = template(
            &world,
            &company.boss,
            Some(company.acme),
            &name,
            "environment",
        )
        .await;
        made.push((id, name));
    }
    made.sort_unstable_by_key(|(id, _)| *id);
    let (late, marker) = made.last().unwrap();
    let late = *late;
    let ordinary = shown(list(&world, &company.boss.token, &Default::default()).await);
    assert_eq!(ids(&ordinary).len(), 500);
    assert!(!ids(&ordinary).contains(&late));
    let legacy = issue(&world, company.boss.id(), None).await;
    assert_eq!(
        shown(list(&world, &legacy.secret, &Default::default()).await),
        ordinary
    );
    let token = key(
        &world,
        company.boss.id(),
        vec![
            rule("TemplateList", "template", late),
            rule("TemplateList", "org", company.acme),
        ],
    )
    .await;
    let selected = shown(list(&world, &token.secret, &Default::default()).await);
    assert_eq!(ids(&selected), [late]);
    assert_eq!(row(&selected, late)["spec"]["marker"], marker.as_str());
}

#[tokio::test]
async fn factual_empty_template_results_still_need_a_selected_read() {
    let s = Scene::new().await;
    let empty = places::make_department(
        &s.world,
        &s.company.boss,
        s.company.acme,
        "Empty department",
        "empty",
    )
    .await
    .base
    .id;
    let query = TemplateListQuery {
        org_id: Some(empty),
        kind: None,
    };
    assert!(ids(&shown(list(&s.world, &s.company.boss.token, &query).await)).is_empty());
    let allowed = key(
        &s.world,
        s.company.boss.id(),
        vec![subtree("TemplateList", s.company.acme)],
    )
    .await;
    assert!(ids(&shown(list(&s.world, &allowed.secret, &query).await)).is_empty());
    for rules in [Vec::new(), vec![subtree("TemplateGet", s.company.acme)]] {
        let token = key(&s.world, s.company.boss.id(), rules).await;
        denied(list(&s.world, &token.secret, &query).await);
    }
}

#[tokio::test]
async fn template_list_restores_current_key_bounds_and_current_home_without_old_rows() {
    let s = Scene::new().await;
    let selected = s.ids[0];
    for revoke in [false, true] {
        let token = key(
            &s.world,
            s.company.boss.id(),
            vec![subtree("TemplateList", s.company.acme)],
        )
        .await;
        assert!(
            ids(&shown(
                list(&s.world, &token.secret, &Default::default()).await
            ))
            .contains(&selected)
        );
        if revoke {
            identity::revoke_api_token(
                &mut *s.world.conn().await,
                token.id,
                s.company.boss.id(),
                s.world.now(),
            )
            .await
            .unwrap();
        } else {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(token.id)
                .bind(json!(KeyScope::default()))
                .execute(&s.world.db.pool)
                .await
                .unwrap();
        }
        let after = list(&s.world, &token.secret, &Default::default()).await;
        assert_eq!(
            after.status,
            if revoke {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::FORBIDDEN
            }
        );
        assert!(!String::from_utf8_lossy(&after.raw).contains("First listed template"));
    }
    let outside = places::make_org(&s.world, &s.company.boss, "Other root", "other-root")
        .await
        .base
        .id;
    let bounded = key(
        &s.world,
        s.company.boss.id(),
        vec![subtree("TemplateList", s.company.acme)],
    )
    .await;
    sqlx::query("UPDATE env_templates SET home_id=$2,org_id=$2 WHERE id=$1")
        .bind(selected)
        .bind(outside)
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    let ordinary = shown(list(&s.world, &s.company.boss.token, &Default::default()).await);
    assert!(ids(&ordinary).contains(&selected));
    assert!(
        !ids(&shown(
            list(&s.world, &bounded.secret, &Default::default()).await
        ))
        .contains(&selected)
    );
}

#[tokio::test]
async fn template_home_and_holder_place_names_require_this_operation_sources() {
    let s = Scene::new().await;
    let id = s.ids[0];
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
        "template",
        id,
        "template.view",
        ("role", Some(topic.base.id), Some("viewer")),
    )
    .await;
    resources::name_person(
        &s.world,
        "template",
        id,
        "template.view",
        s.company.viewer.id(),
    )
    .await;
    let ordinary = shown(list(&s.world, &s.company.viewer.token, &Default::default()).await);
    let normal = row(&ordinary, id);
    assert!(
        holder(normal, "role")["at"]["name"]
            .as_str()
            .unwrap()
            .contains("Protected topic")
    );
    let token = key(
        &s.world,
        s.company.viewer.id(),
        vec![
            rule("TemplateList", "template", id),
            rule("TemplateGet", "org", s.company.acme),
            rule("TemplateGet", "project", project.base.id),
            rule("TemplateGet", "topic", topic.base.id),
        ],
    )
    .await;
    let limited = shown(list(&s.world, &token.secret, &Default::default()).await);
    let hidden = row(&limited, id);
    assert!(hidden["home"]["name"].is_null() && hidden["home"]["redacted"] == true);
    assert!(holder(hidden, "role")["at"]["name"].is_null());
    assert!(holder(hidden, "role")["place"].is_null());
    for field in ["id", "name", "address", "owner", "spec"] {
        assert_eq!(hidden[field], normal[field]);
    }
    assert_eq!(
        hidden["access"]["permissions"],
        normal["access"]["permissions"]
    );
    assert_eq!(
        hidden["access"]["through"].as_array().unwrap().len(),
        normal["access"]["through"].as_array().unwrap().len()
    );
    let full = key(
        &s.world,
        s.company.viewer.id(),
        vec![
            rule("TemplateList", "template", id),
            rule("TemplateList", "org", s.company.acme),
            rule("TemplateList", "project", project.base.id),
            rule("TemplateList", "topic", topic.base.id),
        ],
    )
    .await;
    assert_eq!(
        row(
            &shown(list(&s.world, &full.secret, &Default::default()).await),
            id
        ),
        normal
    );
}

#[tokio::test]
async fn template_list_full_group_payload_needs_its_own_read_and_keeps_counts() {
    let s = Scene::new().await;
    let id = s.ids[0];
    let group = s
        .world
        .app
        .call(OrgGroupCreate, &[&s.company.acme])
        .token(&s.company.boss.token)
        .body(&OrgGroupCreateBody {
            name: "Template readers".into(),
            description: Some("Actual full group description".into()),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this ordinary control requires the full native creation response");
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
        "template",
        id,
        "template.view",
        ("group", Some(group.id), None),
    )
    .await;
    let ordinary = shown(list(&s.world, &s.company.viewer.token, &Default::default()).await);
    let expected = &holder(row(&ordinary, id), "group")["group"];
    assert_eq!(expected["member_count"], 2);
    assert_eq!(expected["description"], "Actual full group description");
    for extra in [Vec::new(), vec![rule("TemplateGet", "group", group.id)]] {
        let mut rules = vec![rule("TemplateList", "template", id)];
        rules.extend(extra);
        let token = key(&s.world, s.company.viewer.id(), rules).await;
        denied(list(&s.world, &token.secret, &Default::default()).await);
    }
    let full = key(
        &s.world,
        s.company.viewer.id(),
        vec![
            rule("TemplateList", "template", id),
            rule("TemplateList", "group", group.id),
            rule("TemplateList", "org", s.company.acme),
        ],
    )
    .await;
    let shown = shown(list(&s.world, &full.secret, &Default::default()).await);
    assert_eq!(&holder(row(&shown, id), "group")["group"], expected);
}

#[tokio::test]
async fn minimal_hidden_templates_keep_ordinary_kind_behavior_and_batched_query_cost() {
    let s = Scene::new().await;
    let id = s.ids[0];
    resources::name_person(
        &s.world,
        "template",
        id,
        "access.manage",
        s.company.viewer.id(),
    )
    .await;
    let query = TemplateListQuery {
        org_id: None,
        kind: Some(UnitKind::Provider),
    };
    let ordinary = shown(list(&s.world, &s.company.viewer.token, &query).await);
    let expected = row(&ordinary, id);
    assert_eq!(expected["content_hidden"], true);
    assert_eq!(expected["private"], true);
    assert!(expected.get("spec").is_none());
    let token = key(
        &s.world,
        s.company.viewer.id(),
        vec![rule("TemplateList", "template", id)],
    )
    .await;
    assert_eq!(
        row(&shown(list(&s.world, &token.secret, &query).await), id),
        expected
    );
    async fn measured(s: &Scene) -> (usize, usize) {
        let (reply, count) =
            Statements::measure(list(&s.world, &s.company.boss.token, &Default::default())).await;
        let rows = shown(reply);
        assert!(count > 0, "the actual list statements must be observed");
        (count, ids(&rows).len())
    }
    let (few, few_rows) = measured(&s).await;
    for index in 0..10 {
        template(
            &s.world,
            &s.company.boss,
            Some(s.company.acme),
            &format!("More template {index}"),
            "environment",
        )
        .await;
    }
    let (many, many_rows) = measured(&s).await;
    assert_eq!((few_rows, many_rows), (3, 13));
    assert_eq!(
        few, many,
        "bulk source/name/row queries stay bounded within the same batch"
    );
}

//! ProviderList reuses ordinary candidate policy while bounding actual rows and live projection sources.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod resources;
mod support;

use http::StatusCode;
use orch_access_contract::decisions::{ObjectRef, Scopes};
use orch_access_contract::key_scope::KeyScope;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::directory::{GroupMemberAdd, OrgGroupCreate, OrgGroupCreateBody};
use orch_api_contract::resources::{ProviderList, ProviderListQuery, ProviderListScope};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::identity::{self, NewApiToken};
use orch_db::queries::resources::access_fixtures;
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
    ids: Vec<Id>,
}

async fn insert_provider(world: &World, owner: &Who, home: Id, name: &str) -> Id {
    let mut conn = world.conn().await;
    let id =
        access_fixtures::insert_resource(&mut conn, "provider", Some(owner.id()), Some(home), name)
            .await
            .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Provider.into(),
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
            "First listed provider",
            "Second listed provider",
            "Third listed provider",
        ] {
            ids.push(insert_provider(&world, &company.boss, company.acme, name).await);
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
    json!({"target":{"kind":"org","id":org,"reach":"subtree"},"resources":["provider"],"allow":[{"operation":operation,"actions":["read"]}]})
}
async fn key(world: &World, user: Id, rules: Vec<Json>) -> Key {
    issue(world, user, Some(rules), &[]).await
}
async fn issue(world: &World, user: Id, rules: Option<Vec<Json>>, scopes: &[&str]) -> Key {
    let policy = rules.map(|rules| {
        let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        serde_json::to_value(scope).unwrap()
    });
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    let scopes: Vec<String> = scopes.iter().map(|s| (*s).to_owned()).collect();
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Provider list control",
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
async fn list(world: &World, token: &str, query: &ProviderListQuery) -> RawReply {
    world
        .app
        .call(ProviderList, &[])
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
fn ids(page: &Json) -> Vec<Id> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| serde_json::from_value(item["id"].clone()).unwrap())
        .collect()
}
fn row<'a>(page: &'a Json, id: Id) -> &'a Json {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == json!(id))
        .expect("the actual admitted row remains")
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
fn all() -> ProviderListQuery {
    ProviderListQuery {
        limit: Some(200),
        ..Default::default()
    }
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
        .expect("the real holder remains")
}

#[tokio::test]
async fn exact_provider_read_filters_before_paging_and_other_operations_do_not_lend_their_rows() {
    let s = Scene::new().await;
    let ordinary = shown(list(&s.world, &s.company.boss.token, &all()).await);
    assert_eq!(ids(&ordinary), s.ids);
    let admitted = [s.ids[1], s.ids[2]];
    let token = key(
        &s.world,
        s.company.boss.id(),
        admitted
            .iter()
            .map(|id| rule("ProviderList", "provider", *id))
            .collect(),
    )
    .await;
    let mut query = ProviderListQuery {
        limit: Some(1),
        ..Default::default()
    };
    let mut found = Vec::new();
    let mut cursors = std::collections::BTreeSet::new();
    loop {
        let page = shown(list(&s.world, &token.secret, &query).await);
        assert_eq!(
            ids(&page).len(),
            1,
            "scope-excluded earlier rows do not consume the page"
        );
        found.extend(ids(&page));
        let Some(cursor) = page["next_cursor"].as_str() else {
            break;
        };
        assert!(cursors.insert(cursor.to_owned()));
        query.cursor = Some(cursor.to_owned());
    }
    assert_eq!(found, admitted);
    let wrong = key(
        &s.world,
        s.company.boss.id(),
        vec![rule("ProviderGet", "provider", s.ids[1])],
    )
    .await;
    denied(list(&s.world, &wrong.secret, &all()).await);
    let stranger = key(
        &s.world,
        s.company.stranger.id(),
        vec![rule("ProviderList", "provider", s.ids[0])],
    )
    .await;
    let browser = shown(list(&s.world, &s.company.stranger.token, &all()).await);
    assert!(ids(&browser).is_empty());
    assert!(
        ids(&shown(list(&s.world, &stranger.secret, &all()).await)).is_empty(),
        "scope does not manufacture ordinary provider visibility"
    );
}

#[tokio::test]
async fn ordinary_scope_modes_and_subscription_sharing_are_preserved_for_browser_and_legacy_keys() {
    let s = Scene::new().await;
    let user = s.company.viewer.id();
    let own = insert_provider(
        &s.world,
        &s.company.viewer,
        s.company.acme,
        "Viewer owns this",
    )
    .await;
    resources::name_person(&s.world, "provider", s.ids[0], "provider.use", user).await;
    resources::name_person(&s.world, "provider", s.ids[1], "provider.manage", user).await;
    resources::name_person(&s.world, "provider", s.ids[2], "provider.view", user).await;
    let native = insert_provider(
        &s.world,
        &s.company.boss,
        s.company.acme,
        "Subscription subject to sharing",
    )
    .await;
    let mut conn = s.world.conn().await;
    let agent = access_fixtures::insert_resource(
        &mut conn,
        "agent",
        Some(s.company.boss.id()),
        Some(s.company.acme),
        "Subscription profile",
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE providers SET kind='subscription',agent_id=$2,harness_kind='fake' WHERE id=$1",
    )
    .bind(native)
    .bind(agent)
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    resources::name_person(&s.world, "provider", native, "provider.use", user).await;
    s.world
        .set_at(
            orch_settings::Target::Node(s.company.acme),
            "providers.subscription_sharing",
            json!([]),
        )
        .await;
    let token = key(
        &s.world,
        user,
        vec![
            subtree("ProviderList", s.company.acme),
            rule("ProviderList", "org", s.company.acme),
        ],
    )
    .await;
    // Full DTO equality compares equal ordinary authority; the bounded legacy key below
    // separately proves that its ungranted capability labels do not appear in access.permissions.
    let legacy = issue(&s.world, user, None, &["*"]).await;
    let legacy_labels = [
        "provider.view",
        "provider.use",
        "provider.manage",
        "org.view",
        "project.view",
        "topic.view",
        "thread.view",
    ];
    let restricted_legacy = issue(&s.world, user, None, &legacy_labels).await;
    let legacy_scope = Scopes::parse(&legacy_labels).unwrap();
    for scope in [
        ProviderListScope::All,
        ProviderListScope::Viewable,
        ProviderListScope::Mine,
        ProviderListScope::Managed,
        ProviderListScope::Usable,
    ] {
        let query = ProviderListQuery {
            scope: Some(scope),
            ..all()
        };
        let ordinary = shown(list(&s.world, &s.company.viewer.token, &query).await);
        assert_eq!(shown(list(&s.world, &token.secret, &query).await), ordinary);
        assert_eq!(
            shown(list(&s.world, &legacy.secret, &query).await),
            ordinary
        );
        if scope == ProviderListScope::All {
            let restricted = shown(list(&s.world, &restricted_legacy.secret, &query).await);
            let mut expected = ordinary.clone();
            for provider in expected["items"].as_array_mut().unwrap() {
                if let Some(permissions) = provider
                    .get_mut("access")
                    .and_then(|access| access.get_mut("permissions"))
                    .and_then(Json::as_array_mut)
                {
                    permissions.retain(|value| {
                        legacy_scope.allow(value.as_str().unwrap().parse().unwrap())
                    });
                }
            }
            assert_eq!(
                restricted, expected,
                "legacy labels keep their exact permission ceiling"
            );
        }
        let got = ids(&ordinary);
        if scope == ProviderListScope::Mine {
            assert_eq!(got, [own]);
        }
        if scope == ProviderListScope::Managed {
            assert!(got.contains(&own) && got.contains(&s.ids[1]) && !got.contains(&s.ids[0]));
        }
        if scope == ProviderListScope::Usable {
            assert!(
                got.contains(&own)
                    && got.contains(&s.ids[0])
                    && got.contains(&s.ids[1])
                    && !got.contains(&s.ids[2])
                    && !got.contains(&native)
            );
        }
    }
    s.world
        .set_at(
            orch_settings::Target::Node(s.company.acme),
            "providers.subscription_sharing",
            json!(["fake"]),
        )
        .await;
    let query = ProviderListQuery {
        scope: Some(ProviderListScope::Usable),
        ..all()
    };
    let ordinary = shown(list(&s.world, &s.company.viewer.token, &query).await);
    assert!(ids(&ordinary).contains(&native));
    assert_eq!(shown(list(&s.world, &token.secret, &query).await), ordinary);
}

#[tokio::test]
async fn organization_filter_and_subtree_scope_follow_the_current_provider_home() {
    let s = Scene::new().await;
    let inside = places::make_department(
        &s.world,
        &s.company.boss,
        s.company.acme,
        "Research",
        "research",
    )
    .await
    .base
    .id;
    let external = places::make_org(&s.world, &s.company.boss, "Another root", "another-root")
        .await
        .base
        .id;
    let moved = insert_provider(&s.world, &s.company.boss, inside, "Movable provider").await;
    let token = key(
        &s.world,
        s.company.boss.id(),
        vec![subtree("ProviderList", s.company.acme)],
    )
    .await;
    let query = ProviderListQuery {
        org_id: Some(inside),
        ..all()
    };
    assert_eq!(
        ids(&shown(list(&s.world, &s.company.boss.token, &query).await)),
        [moved]
    );
    assert_eq!(
        ids(&shown(list(&s.world, &token.secret, &query).await)),
        [moved]
    );
    sqlx::query(
        "UPDATE providers SET home_id=$2,org_id=(SELECT org_id FROM nodes WHERE id=$2) WHERE id=$1",
    )
    .bind(moved)
    .bind(external)
    .execute(&s.world.db.pool)
    .await
    .unwrap();
    assert!(!ids(&shown(list(&s.world, &token.secret, &all()).await)).contains(&moved));
    assert!(ids(&shown(list(&s.world, &token.secret, &query).await)).is_empty());
    assert!(
        ids(&shown(list(&s.world, &s.company.boss.token, &all()).await)).contains(&moved),
        "the ordinary owner still sees the moved identity"
    );
}

#[tokio::test]
async fn checked_empty_lists_require_a_real_selected_read_without_borrowing_another_operation() {
    let s = Scene::new().await;
    let none = key(&s.world, s.company.boss.id(), Vec::new()).await;
    let wrong = key(
        &s.world,
        s.company.boss.id(),
        vec![subtree("ProviderGet", s.company.acme)],
    )
    .await;
    let permitted = key(
        &s.world,
        s.company.boss.id(),
        vec![subtree("ProviderList", s.company.acme)],
    )
    .await;
    let query = ProviderListQuery {
        scope: Some(ProviderListScope::Mine),
        org_id: Some(
            places::make_department(
                &s.world,
                &s.company.boss,
                s.company.acme,
                "Empty department",
                "empty",
            )
            .await
            .base
            .id,
        ),
        ..all()
    };
    assert!(ids(&shown(list(&s.world, &s.company.boss.token, &query).await)).is_empty());
    assert!(ids(&shown(list(&s.world, &permitted.secret, &query).await)).is_empty());
    for token in [&none.secret, &wrong.secret] {
        denied(list(&s.world, token, &query).await);
    }
}

#[tokio::test]
async fn list_live_home_role_and_domain_names_are_same_operation_sources_not_new_account_reads() {
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
        "provider",
        id,
        "provider.view",
        ("role", Some(topic.base.id), Some("viewer")),
    )
    .await;
    resources::name_person(
        &s.world,
        "provider",
        id,
        "provider.view",
        s.company.viewer.id(),
    )
    .await;
    let ordinary = shown(list(&s.world, &s.company.viewer.token, &all()).await);
    let normal = row(&ordinary, id);
    assert!(
        holder(normal, "role")["at"]["name"]
            .as_str()
            .unwrap()
            .contains("Protected topic")
    );
    let narrow = key(
        &s.world,
        s.company.viewer.id(),
        vec![
            rule("ProviderList", "provider", id),
            rule("ProviderGet", "org", s.company.acme),
            rule("ProviderGet", "project", project.base.id),
            rule("ProviderGet", "topic", topic.base.id),
        ],
    )
    .await;
    let page = shown(list(&s.world, &narrow.secret, &all()).await);
    let hidden = row(&page, id);
    assert!(hidden["home"]["name"].is_null() && hidden["home"]["redacted"] == true);
    assert!(holder(hidden, "role")["at"]["name"].is_null());
    assert!(holder(hidden, "role")["place"].is_null());
    assert_eq!(holder(hidden, "user")["user"]["address"], "@viewer");
    for field in ["id", "name", "address", "owner", "owners"] {
        assert_eq!(
            hidden[field], normal[field],
            "provider and public owner identity {field}"
        );
    }
    let full = key(
        &s.world,
        s.company.viewer.id(),
        vec![
            rule("ProviderList", "provider", id),
            rule("ProviderList", "org", s.company.acme),
            rule("ProviderList", "project", project.base.id),
            rule("ProviderList", "topic", topic.base.id),
        ],
    )
    .await;
    assert_eq!(
        row(&shown(list(&s.world, &full.secret, &all()).await), id),
        normal
    );
    assert_eq!(
        hidden["access"]["permissions"],
        normal["access"]["permissions"]
    );
    assert_eq!(
        hidden["access"]["through"].as_array().unwrap().len(),
        normal["access"]["through"].as_array().unwrap().len()
    );
}

#[tokio::test]
async fn actual_full_group_payload_requires_this_list_operations_group_read_and_keeps_counts() {
    let s = Scene::new().await;
    let id = s.ids[0];
    let group = s
        .world
        .app
        .call(OrgGroupCreate, &[&s.company.acme])
        .token(&s.company.boss.token)
        .body(&OrgGroupCreateBody {
            name: "Listing reader group".into(),
            description: Some("Stored full group description".into()),
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
        "provider",
        id,
        "provider.view",
        ("group", Some(group.id), None),
    )
    .await;
    let ordinary = shown(list(&s.world, &s.company.viewer.token, &all()).await);
    let group_payload = &holder(row(&ordinary, id), "group")["group"];
    assert_eq!(group_payload["member_count"], 2);
    assert_eq!(
        group_payload["description"],
        "Stored full group description"
    );
    let primary = rule("ProviderList", "provider", id);
    for source in [Vec::new(), vec![rule("ProviderGet", "group", group.id)]] {
        let mut rules = vec![primary.clone()];
        rules.extend(source);
        let token = key(&s.world, s.company.viewer.id(), rules).await;
        denied(list(&s.world, &token.secret, &all()).await);
    }
    let token = key(
        &s.world,
        s.company.viewer.id(),
        vec![
            primary,
            rule("ProviderList", "group", group.id),
            rule("ProviderList", "org", s.company.acme),
        ],
    )
    .await;
    let page = shown(list(&s.world, &token.secret, &all()).await);
    assert_eq!(&holder(row(&page, id), "group")["group"], group_payload);
    assert_eq!(row(&page, id)["owner"], row(&ordinary, id)["owner"]);
}

#[tokio::test]
async fn page_management_only_rows_keep_the_existing_minimal_hidden_shape() {
    let s = Scene::new().await;
    let id = s.ids[0];
    resources::name_person(
        &s.world,
        "provider",
        id,
        "access.manage",
        s.company.viewer.id(),
    )
    .await;
    let ordinary = shown(list(&s.world, &s.company.viewer.token, &all()).await);
    let expected = row(&ordinary, id);
    assert_eq!(expected["content_hidden"], true);
    assert_eq!(expected["private"], true);
    assert!(expected.get("status").is_none());
    let token = key(
        &s.world,
        s.company.viewer.id(),
        vec![rule("ProviderList", "provider", id)],
    )
    .await;
    let page = shown(list(&s.world, &token.secret, &all()).await);
    assert_eq!(row(&page, id), expected);
    let other = key(
        &s.world,
        s.company.viewer.id(),
        vec![rule("ProviderList", "provider", s.ids[1])],
    )
    .await;
    assert!(!ids(&shown(list(&s.world, &other.secret, &all()).await)).contains(&id));
}

#[tokio::test]
async fn list_reads_restore_current_persisted_scope_and_revocation_without_leaking_old_rows() {
    let s = Scene::new().await;
    for revoke in [false, true] {
        let token = key(
            &s.world,
            s.company.boss.id(),
            vec![subtree("ProviderList", s.company.acme)],
        )
        .await;
        assert_eq!(
            ids(&shown(list(&s.world, &token.secret, &all()).await)),
            s.ids
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
                .bind(serde_json::to_value(KeyScope::default()).unwrap())
                .execute(&s.world.db.pool)
                .await
                .unwrap();
        }
        let after = list(&s.world, &token.secret, &all()).await;
        assert_eq!(
            after.status,
            if revoke {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::FORBIDDEN
            }
        );
        assert!(!String::from_utf8_lossy(&after.raw).contains("First listed provider"));
    }
}

//! Native node lists fill scoped windows without changing account ACL or DTO meaning.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use crate::support::FullPermissionPageReply;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::access::PermissionPageUpdate;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::NodeDocumentCreate;
use orch_api_contract::nodes::{
    NodeChildList, NodeChildListQuery, NodeKind, NodeUpdate, TopicList, TopicListQuery, TopicSort,
};
use orch_api_contract::ownership::NodeDelete;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::{hierarchy, identity, resources, threads};
use orch_types::{Id, Json, new_id};
use places::{Who, join, make_department, make_org, make_project, make_topic, person};
use serde_json::json;
use support::World;

#[derive(Clone, Copy, Debug)]
enum List {
    Children,
    Topics,
}
impl List {
    const ALL: [Self; 2] = [Self::Children, Self::Topics];
    fn operation(self) -> &'static str {
        match self {
            Self::Children => "NodeChildList.read",
            Self::Topics => "TopicList",
        }
    }
}
struct Scene {
    world: World,
    owner: Who,
    org: Id,
    project: Id,
    topics: [Id; 3],
}
impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        let owner = person(&world, "owner").await;
        let org = make_org(&world, &owner, "List office", "list-office")
            .await
            .base
            .id;
        let project = make_project(&world, &owner, org, "Existing parent project name")
            .await
            .base
            .id;
        let a = make_topic(&world, &owner, project, "A unselected topic content")
            .await
            .base
            .id;
        let b = make_topic(&world, &owner, project, "B selected topic content")
            .await
            .base
            .id;
        let c = make_topic(&world, &owner, project, "C selected topic content")
            .await
            .base
            .id;
        Self {
            world,
            owner,
            org,
            project,
            topics: [a, b, c],
        }
    }
    async fn key(&self, operation: &str, ids: &[Id], extra: Vec<Json>) -> String {
        let mut rules: Vec<_> = ids.iter().map(|id| rule(operation, "topic", *id)).collect();
        rules.extend(extra);
        key(&self.world, self.owner.id(), rules).await
    }
}
fn rule(operation: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":["read"]}]})
}
async fn key(world: &World, owner: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: owner,
            name: "Native node list control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}
async fn fetch(
    s: &Scene,
    which: List,
    token: &str,
    limit: u32,
    cursor: Option<String>,
) -> RawReply {
    match which {
        List::Children => {
            s.world
                .app
                .call(NodeChildList, &[&s.project])
                .token(token)
                .query(&NodeChildListQuery {
                    kind: Some(NodeKind::Topic),
                    limit: Some(limit),
                    cursor,
                    ..Default::default()
                })
                .send_bytes()
                .await
        }
        List::Topics => {
            s.world
                .app
                .call(TopicList, &[&s.project])
                .token(token)
                .query(&TopicListQuery {
                    sort: Some(TopicSort::Name),
                    limit: Some(limit),
                    cursor,
                    ..Default::default()
                })
                .send_bytes()
                .await
        }
    }
}
fn success(reply: RawReply) -> Json {
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
        .map(|row| serde_json::from_value(row["id"].clone()).unwrap())
        .collect()
}

#[tokio::test]
async fn both_native_lists_fill_pages_after_scope_filtering_and_do_not_loan_parent_names() {
    let mut failures = Vec::new();
    for which in List::ALL {
        let s = Scene::new().await;
        let ordinary = success(fetch(&s, which, &s.owner.token, 20, None).await);
        let ordinary_ids = ids(&ordinary);
        assert_eq!(ordinary_ids.len(), 3);
        let selected = &ordinary_ids[1..];
        let token = s.key(which.operation(), selected, vec![]).await;
        let first = fetch(&s, which, &token, 1, None).await;
        if first.status != StatusCode::OK {
            failures.push(format!(
                "{} expected200 got{} {}",
                which.operation(),
                first.status,
                String::from_utf8_lossy(&first.raw)
            ));
            continue;
        }
        let first = success(first);
        assert_eq!(ids(&first), vec![selected[0]]);
        let path = first["items"][0]["path"].as_array().unwrap();
        let parent = path
            .iter()
            .find(|node| node["id"] == json!(s.project))
            .unwrap();
        assert_eq!(parent["redacted"], true);
        assert!(parent.get("name").is_none_or(Json::is_null));
        let cursor = first["next_cursor"]
            .as_str()
            .expect("a second selected row exists")
            .to_owned();
        let second = success(fetch(&s, which, &token, 1, Some(cursor.clone())).await);
        assert_eq!(ids(&second), vec![selected[1]]);
        assert!(second["next_cursor"].is_null());
        let only_first = s.key(which.operation(), &selected[..1], vec![]).await;
        let exhausted = success(fetch(&s, which, &only_first, 1, Some(cursor)).await);
        assert!(ids(&exhausted).is_empty());
        assert!(exhausted["next_cursor"].is_null());
        assert!(!first.to_string().contains("A unselected topic content"));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn mixed_child_kinds_bind_their_actual_business_families_without_resource_cross_product() {
    let s = Scene::new().await;
    let department = make_department(
        &s.world,
        &s.owner,
        s.org,
        "Unselected department content",
        "unselected-department",
    )
    .await
    .base
    .id;
    let ordinary = success(
        s.world
            .app
            .call(NodeChildList, &[&s.org])
            .token(&s.owner.token)
            .query(&NodeChildListQuery {
                limit: Some(20),
                ..Default::default()
            })
            .send_bytes()
            .await,
    );
    assert!(ids(&ordinary).contains(&department));
    assert!(ids(&ordinary).contains(&s.project));
    let token=key(&s.world,s.owner.id(),vec![json!({"target":{"kind":"org","id":s.org,"reach":"subtree"},"resources":["project"],"allow":[{"operation":"NodeChildList.read","actions":["read"]}]})]).await;
    let page = success(
        s.world
            .app
            .call(NodeChildList, &[&s.org])
            .token(&token)
            .query(&NodeChildListQuery {
                limit: Some(20),
                ..Default::default()
            })
            .send_bytes()
            .await,
    );
    // OrgCreate also makes its native General project; both are Project resources.
    let ordinary_projects: Vec<Id> = ordinary["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["kind"] == "project")
        .map(|row| serde_json::from_value(row["id"].clone()).unwrap())
        .collect();
    assert_eq!(ids(&page), ordinary_projects);
    assert!(!ids(&page).contains(&department));
    assert!(!page.to_string().contains("Unselected department content"));
    let token = key(
        &s.world,
        s.owner.id(),
        vec![
            rule("NodeChildList.read", "org", department),
            rule("NodeChildList.read", "project", s.project),
        ],
    )
    .await;
    let mixed = success(
        s.world
            .app
            .call(NodeChildList, &[&s.org])
            .token(&token)
            .query(&NodeChildListQuery {
                limit: Some(20),
                ..Default::default()
            })
            .send_bytes()
            .await,
    );
    let expected: Vec<_> = ids(&ordinary)
        .into_iter()
        .filter(|id| *id == department || *id == s.project)
        .collect();
    assert_eq!(ids(&mixed), expected);
    assert!(
        mixed["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == json!(department) && row["kind"] == "org")
    );
}

#[tokio::test]
async fn scope_filters_existing_private_metadata_rows_without_granting_content_view() {
    let s = Scene::new().await;
    let viewer = person(&s.world, "viewer").await;
    join(&s.world, s.org, s.org, &viewer, "viewer", &s.owner).await;
    let private = s.topics[1];
    s.world.app.call(PermissionPageUpdate,&[&"nodes",&private]).token(&s.owner.token)
        .raw_body(&json!({"whole":"private","rows":[{"permission":"access.manage","add":[{"holder":{"kind":"user","id":viewer.id()}}]}]})).send().await.full_permission_page().ok();
    for which in List::ALL {
        let ordinary = success(fetch(&s, which, &viewer.token, 20, None).await);
        let row = ordinary["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == json!(private))
            .expect("the native private-page metadata row exists");
        assert_eq!(row["content_hidden"], true);
        assert_eq!(row["private"], true);
        let token = key(
            &s.world,
            viewer.id(),
            vec![rule(which.operation(), "topic", private)],
        )
        .await;
        let page = success(fetch(&s, which, &token, 20, None).await);
        assert_eq!(ids(&page), vec![private]);
        assert_eq!(page["items"][0]["content_hidden"], true);
        assert!(page["items"][0].get("thread_count").is_none());
        let token = key(
            &s.world,
            viewer.id(),
            vec![rule(which.operation(), "topic", s.topics[0])],
        )
        .await;
        let page = success(fetch(&s, which, &token, 20, None).await);
        assert!(!ids(&page).contains(&private));
        assert!(!page.to_string().contains("B selected topic content"));
    }
}

#[tokio::test]
async fn node_child_retained_variant_uses_its_own_manage_selection_and_native_restore_guard() {
    let s = Scene::new().await;
    let deleted = s.topics[0];
    success(
        s.world
            .app
            .call(NodeDelete, &[&deleted])
            .token(&s.owner.token)
            .raw_body(&json!({}))
            .send_bytes()
            .await,
    );
    let query = NodeChildListQuery {
        inspect_deleted: Some(true),
        kind: Some(NodeKind::Topic),
        limit: Some(20),
        ..Default::default()
    };
    let ordinary = success(
        s.world
            .app
            .call(NodeChildList, &[&s.project])
            .token(&s.owner.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    let retained = ordinary["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == json!(deleted))
        .expect("the native retained child is inspectable");
    assert_eq!(retained["inspection"]["readonly"], true);
    let token=key(&s.world,s.owner.id(),vec![json!({"target":{"kind":"topic","id":deleted,"reach":"self"},"allow":[{"operation":"NodeChildList.inspect_deleted","actions":["manage"]}]})]).await;
    let page = success(
        s.world
            .app
            .call(NodeChildList, &[&s.project])
            .token(&token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(ids(&page), vec![deleted]);
    assert_eq!(page["items"][0]["inspection"]["readonly"], true);
    let unrelated = s.key("NodeChildList.read", &[deleted], vec![]).await;
    assert_eq!(
        s.world
            .app
            .call(NodeChildList, &[&s.project])
            .token(&unrelated)
            .query(&query)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let viewer = person(&s.world, "inspection-viewer").await;
    join(&s.world, s.org, s.org, &viewer, "viewer", &s.owner).await;
    let ordinary_viewer = success(
        s.world
            .app
            .call(NodeChildList, &[&s.project])
            .token(&viewer.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert!(
        !ids(&ordinary_viewer).contains(&deleted),
        "native restoration authority excludes this retained child"
    );
    let viewer_key = key(
        &s.world,
        viewer.id(),
        vec![json!({
            "target":{"kind":"topic","id":deleted,"reach":"self"},
            "allow":[{"operation":"NodeChildList.inspect_deleted","actions":["manage"]}]
        })],
    )
    .await;
    let bounded_viewer = success(
        s.world
            .app
            .call(NodeChildList, &[&s.project])
            .token(&viewer_key)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert!(ids(&bounded_viewer).is_empty());
}

#[tokio::test]
async fn lists_keep_optional_readme_and_complete_environment_payloads_under_the_same_operation() {
    let s = Scene::new().await;
    let with_env = s.topics[0];
    let with_doc = s.topics[1];
    let environment = resources::access_fixtures::insert_environment(
        &mut *s.world.conn().await,
        s.owner.id(),
        Some(with_env),
        "Existing environment reference content",
        "managed",
    )
    .await
    .unwrap();
    let thread = new_id();
    assert!(
        threads::insert_thread(
            &mut *s.world.conn().await,
            thread,
            with_env,
            "Associated idle thread",
            s.owner.id(),
            s.world.now()
        )
        .await
        .unwrap()
    );
    threads::set_environment(
        &mut *s.world.conn().await,
        thread,
        environment,
        "/workspace",
        1,
    )
    .await
    .unwrap();
    let created = s
        .world
        .app
        .call(NodeDocumentCreate, &[&with_doc])
        .token(&s.owner.token)
        .raw_body(
            &json!({"title":"Existing README reference content","markdown":"Existing paragraph"}),
        )
        .send_bytes()
        .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let document: Json = serde_json::from_slice(&created.raw).unwrap();
    let doc_id: Id = serde_json::from_value(document["id"].clone()).unwrap();
    let revision = hierarchy::with_ancestors(&mut *s.world.conn().await, &[with_doc])
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == with_doc)
        .unwrap()
        .revision;
    success(
        s.world
            .app
            .call(NodeUpdate, &[&with_doc])
            .token(&s.owner.token)
            .raw_body(&json!({"expected_revision":revision,"readme_document_id":doc_id}))
            .send_bytes()
            .await,
    );
    for which in List::ALL {
        let ordinary = success(fetch(&s, which, &s.owner.token, 20, None).await);
        let rows = ordinary["items"].as_array().unwrap();
        let env_row = rows
            .iter()
            .find(|row| row["id"] == json!(with_env))
            .unwrap();
        let doc_row = rows
            .iter()
            .find(|row| row["id"] == json!(with_doc))
            .unwrap();
        assert!(
            env_row["environments"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == json!(environment))
        );
        assert_eq!(doc_row["readme"]["id"], json!(doc_id));
        let token = s
            .key(which.operation(), &[with_env, with_doc], vec![])
            .await;
        let page = success(fetch(&s, which, &token, 20, None).await);
        assert_eq!(ids(&page), vec![with_doc]);
        assert!(page["items"][0].get("readme").is_none_or(Json::is_null));
        let token = s
            .key(
                which.operation(),
                &[with_env, with_doc],
                vec![
                    rule("EnvironmentGet", "environment", environment),
                    rule("DocumentGet", "document", doc_id),
                ],
            )
            .await;
        assert_eq!(
            ids(&success(fetch(&s, which, &token, 20, None).await)),
            vec![with_doc]
        );
        let token = s
            .key(
                which.operation(),
                &[with_env, with_doc],
                vec![
                    rule(which.operation(), "environment", environment),
                    rule(which.operation(), "document", doc_id),
                ],
            )
            .await;
        let page = success(fetch(&s, which, &token, 20, None).await);
        assert_eq!(ids(&page).len(), 2);
        let rows = page["items"].as_array().unwrap();
        let env_row = rows
            .iter()
            .find(|row| row["id"] == json!(with_env))
            .unwrap();
        let doc_row = rows
            .iter()
            .find(|row| row["id"] == json!(with_doc))
            .unwrap();
        assert_eq!(
            env_row["environments"][0]["name"],
            "Existing environment reference content"
        );
        assert_eq!(
            doc_row["readme"]["title"],
            "Existing README reference content"
        );
    }
}

#[tokio::test]
async fn thread_children_keep_native_payload_and_bound_their_optional_parent_name() {
    let s = Scene::new().await;
    let parent = new_id();
    assert!(
        threads::insert_thread(
            &mut *s.world.conn().await,
            parent,
            s.topics[0],
            "Existing parent thread content",
            s.owner.id(),
            s.world.now()
        )
        .await
        .unwrap()
    );
    let side = new_id();
    assert!(
        threads::forking::insert_side_thread(
            &mut *s.world.conn().await,
            &threads::forking::NewSide {
                id: side,
                parent,
                name: "Existing side thread content",
                owner: s.owner.id(),
                from_turn: 0,
                at: s.world.now(),
            }
        )
        .await
        .unwrap()
    );
    let query = NodeChildListQuery {
        kind: Some(NodeKind::Thread),
        limit: Some(20),
        ..Default::default()
    };
    let ordinary = success(
        s.world
            .app
            .call(NodeChildList, &[&parent])
            .token(&s.owner.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(ids(&ordinary), vec![side]);
    assert_eq!(
        ordinary["items"][0]["side_of"]["name"],
        "Existing parent thread content"
    );
    let selected = rule("NodeChildList.read", "thread", side);
    let token = key(
        &s.world,
        s.owner.id(),
        vec![
            selected.clone(),
            rule("ThreadSyncGet.read", "thread", parent),
        ],
    )
    .await;
    let bounded = success(
        s.world
            .app
            .call(NodeChildList, &[&parent])
            .token(&token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(ids(&bounded), vec![side]);
    assert_eq!(bounded["items"][0]["kind"], "thread");
    assert_eq!(bounded["items"][0]["side_of"]["id"], json!(parent));
    assert!(bounded["items"][0]["side_of"]["name"].is_null());
    assert_eq!(bounded["items"][0]["side_of"]["redacted"], true);
    assert!(
        !bounded
            .to_string()
            .contains("Existing parent thread content")
    );
    let token = key(
        &s.world,
        s.owner.id(),
        vec![selected, rule("NodeChildList.read", "thread", parent)],
    )
    .await;
    let complete = success(
        s.world
            .app
            .call(NodeChildList, &[&parent])
            .token(&token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(
        complete["items"][0]["side_of"]["name"],
        "Existing parent thread content"
    );
}

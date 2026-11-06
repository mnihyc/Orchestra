//! Native personal ownership and subject facts use the shared stored/selected read adapters.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

mod places;
mod support;

use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::SubscriptionLevel;
use orch_api_contract::nodes::{NodeSubscriptionSet, NodeSubscriptionSetBody};
use orch_api_contract::notifications::PersonNotifyContextGet;
use orch_api_contract::personal::{
    MeFavoriteList, MeFavoriteSet, MeFavoriteSetBody, MeReadStateList, MeReadStateListQuery,
    MeSubscriptionList, MeSubscriptionListQuery,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::{hierarchy, identity};
use orch_types::{Id, Json, new_id};
use places::Who;
use serde_json::json;
use support::World;

fn rule(operation: &str, kind: &str, id: Id, resource: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[resource],
        "allow":[{"operation":operation,"actions":["read"]}]})
}

async fn key(world: &World, actor: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: actor,
            name: "Personal native scope fixture",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    secret
}

fn checked(reply: RawReply, status: StatusCode) -> Json {
    assert_eq!(
        reply.status,
        status,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

struct Scene {
    world: World,
    owner: Who,
    org: Id,
    first: Id,
    second: Id,
}

impl Scene {
    async fn new() -> Self {
        let world = World::new().await;
        let owner = places::person(&world, "owner").await;
        let org = places::make_org(&world, &owner, "Personal root", "personal-root")
            .await
            .base
            .id;
        let first = places::make_project(&world, &owner, org, "Selected project")
            .await
            .base
            .id;
        let second = places::make_project(&world, &owner, org, "Unselected project")
            .await
            .base
            .id;
        Self {
            world,
            owner,
            org,
            first,
            second,
        }
    }
    async fn read_thread(&self, parent: Id, seq: i64) -> Id {
        let thread = new_id();
        hierarchy::testing::insert_node(
            &mut *self.world.conn().await,
            thread,
            "thread",
            Some(parent),
            "Read position",
            self.owner.id(),
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO thread_reads(user_id,thread_id,last_read_seq) VALUES($1,$2,$3)")
            .bind(self.owner.id())
            .bind(thread)
            .bind(seq)
            .execute(&self.world.db.pool)
            .await
            .unwrap();
        let live: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM threads t JOIN thread_continuations c ON c.thread_id=t.node_id AND c.id=t.active_continuation_id WHERE t.node_id=$1)")
            .bind(thread).fetch_one(&self.world.db.pool).await.unwrap();
        assert!(
            live,
            "fixture must have the actual native active continuation"
        );
        thread
    }
}

#[tokio::test]
async fn favorites_keep_native_order_with_own_operation_protected_path_names() {
    let s = Scene::new().await;
    for node in [s.first, s.second] {
        s.world
            .app
            .call(MeFavoriteSet, &[&node])
            .token(&s.owner.token)
            .body(&MeFavoriteSetBody {
                after_node_id: None,
                view: None,
            })
            .send()
            .await
            .ok();
    }
    let selected = key(
        &s.world,
        s.owner.id(),
        vec![
            rule("MeFavoriteList", "account", s.owner.id(), "account"),
            rule("MeFavoriteList", "project", s.second, "project"),
            rule("NodeTagList", "org", s.org, "org"),
        ],
    )
    .await;
    let response = checked(
        s.world
            .app
            .call(MeFavoriteList, &[])
            .token(&selected)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(response.as_array().unwrap().len(), 1);
    assert_eq!(response[0]["node"]["id"], json!(s.second));
    assert_eq!(response[0]["node"]["name"], "Unselected project");
    assert!(
        !response.to_string().contains("Personal root"),
        "another read operation cannot lend a protected ancestor name"
    );
    let full = key(
        &s.world,
        s.owner.id(),
        vec![
            rule("MeFavoriteList", "account", s.owner.id(), "account"),
            rule("MeFavoriteList", "project", s.first, "project"),
            rule("MeFavoriteList", "project", s.second, "project"),
            rule("MeFavoriteList", "org", s.org, "org"),
        ],
    )
    .await;
    let ordinary = checked(
        s.world
            .app
            .call(MeFavoriteList, &[])
            .token(&s.owner.token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    let scoped = checked(
        s.world
            .app
            .call(MeFavoriteList, &[])
            .token(&full)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(scoped, ordinary);
    let empty = key(
        &s.world,
        s.owner.id(),
        vec![
            rule("MeFavoriteList", "account", s.owner.id(), "account"),
            rule("MeFavoriteList", "project", new_id(), "project"),
        ],
    )
    .await;
    assert_eq!(
        checked(
            s.world
                .app
                .call(MeFavoriteList, &[])
                .token(&empty)
                .send_bytes()
                .await,
            StatusCode::OK
        ),
        json!([])
    );
}

#[tokio::test]
async fn subscription_rows_require_own_account_and_actual_selected_node_facts() {
    let s = Scene::new().await;
    for node in [s.first, s.second] {
        s.world
            .app
            .call(NodeSubscriptionSet, &[&node])
            .token(&s.owner.token)
            .body(&NodeSubscriptionSetBody {
                level: SubscriptionLevel::All,
            })
            .send()
            .await
            .ok();
    }
    let selected = key(
        &s.world,
        s.owner.id(),
        vec![
            rule("MeSubscriptionList", "account", s.owner.id(), "account"),
            rule("MeSubscriptionList", "project", s.second, "project"),
        ],
    )
    .await;
    let response = checked(
        s.world
            .app
            .call(MeSubscriptionList, &[])
            .token(&selected)
            .query(&MeSubscriptionListQuery {
                org_id: Some(s.org),
            })
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(response.as_array().unwrap().len(), 1);
    assert_eq!(response[0]["node_id"], json!(s.second));
    assert_eq!(response[0]["source"], "manual");
    let no_account = key(
        &s.world,
        s.owner.id(),
        vec![rule("MeSubscriptionList", "project", s.second, "project")],
    )
    .await;
    checked(
        s.world
            .app
            .call(MeSubscriptionList, &[])
            .token(&no_account)
            .query(&MeSubscriptionListQuery::default())
            .send_bytes()
            .await,
        StatusCode::FORBIDDEN,
    );
    let other = places::person(&s.world, "other").await;
    let wrong_owner = key(
        &s.world,
        other.id(),
        vec![
            rule("MeSubscriptionList", "account", other.id(), "account"),
            rule("MeSubscriptionList", "project", s.second, "project"),
        ],
    )
    .await;
    let response = checked(
        s.world
            .app
            .call(MeSubscriptionList, &[])
            .token(&wrong_owner)
            .query(&MeSubscriptionListQuery::default())
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(
        response,
        json!([]),
        "scope does not lend another account's private state"
    );
}

#[tokio::test]
async fn read_positions_bind_the_real_active_thread_and_preserve_empty_filtered_lists() {
    let s = Scene::new().await;
    let topic = places::make_topic(&s.world, &s.owner, s.first, "Read topic")
        .await
        .base
        .id;
    let first = s.read_thread(topic, 7).await;
    let second = s.read_thread(topic, 11).await;
    let selected = key(
        &s.world,
        s.owner.id(),
        vec![
            rule("MeReadStateList", "account", s.owner.id(), "account"),
            rule("MeReadStateList", "thread", second, "thread"),
        ],
    )
    .await;
    let response = checked(
        s.world
            .app
            .call(MeReadStateList, &[])
            .token(&selected)
            .query(&MeReadStateListQuery {
                org_id: Some(s.org),
            })
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(response.as_array().unwrap().len(), 1);
    assert_eq!(response[0]["thread_id"], json!(second));
    assert_eq!(response[0]["last_read_seq"], 11);
    assert!(response[0]["continuation_id"].is_string());
    assert!(!response.to_string().contains(&first.to_string()));
    let empty = checked(
        s.world
            .app
            .call(MeReadStateList, &[])
            .token(&selected)
            .query(&MeReadStateListQuery {
                org_id: Some(s.owner.person.personal_org),
            })
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(empty, json!([]));
    let wrong_operation = key(
        &s.world,
        s.owner.id(),
        vec![
            rule("MeSubscriptionList", "account", s.owner.id(), "account"),
            rule("MeSubscriptionList", "thread", second, "thread"),
        ],
    )
    .await;
    checked(
        s.world
            .app
            .call(MeReadStateList, &[])
            .token(&wrong_operation)
            .query(&MeReadStateListQuery::default())
            .send_bytes()
            .await,
        StatusCode::FORBIDDEN,
    );
}

#[tokio::test]
async fn notification_context_reuses_native_subject_readability_without_granting_or_notifying() {
    let s = Scene::new().await;
    let recipient = places::person(&s.world, "recipient").await;
    let ordinary = checked(
        s.world
            .app
            .call(
                PersonNotifyContextGet,
                &[&"nodes", &s.first, &recipient.id()],
            )
            .token(&s.owner.token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    let selected = key(
        &s.world,
        s.owner.id(),
        vec![rule(
            "PersonNotifyContextGet",
            "project",
            s.first,
            "project",
        )],
    )
    .await;
    let scoped = checked(
        s.world
            .app
            .call(
                PersonNotifyContextGet,
                &[&"nodes", &s.first, &recipient.id()],
            )
            .token(&selected)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(scoped, ordinary);
    let stranger = key(
        &s.world,
        recipient.id(),
        vec![rule(
            "PersonNotifyContextGet",
            "project",
            s.first,
            "project",
        )],
    )
    .await;
    let denied = s
        .world
        .app
        .call(PersonNotifyContextGet, &[&"nodes", &s.first, &s.owner.id()])
        .token(&stranger)
        .send_bytes()
        .await;
    assert_eq!(denied.status, StatusCode::NOT_FOUND);
    let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM jobs),(SELECT count(*) FROM commands),(SELECT count(*) FROM notifications WHERE kind='person.notify')")
        .fetch_one(&s.world.db.pool).await.unwrap();
    assert_eq!(
        counts,
        (0, 0, 0),
        "context inspection has no grant, notification or dispatch effect"
    );
}

#[tokio::test]
async fn notification_context_resource_kinds_come_from_the_native_stored_subject() {
    let s = Scene::new().await;
    let recipient = places::person(&s.world, "recipient").await;
    let template = orch_db::queries::resources::testing::insert_resource(
        &mut *s.world.conn().await,
        "template",
        Some(s.owner.id()),
        Some(s.org),
        "Native context template",
    )
    .await
    .unwrap();
    let ordinary = checked(
        s.world
            .app
            .call(
                PersonNotifyContextGet,
                &[&"templates", &template, &recipient.id()],
            )
            .token(&s.owner.token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    let selected = key(
        &s.world,
        s.owner.id(),
        vec![rule(
            "PersonNotifyContextGet",
            "template",
            template,
            "template",
        )],
    )
    .await;
    let response = checked(
        s.world
            .app
            .call(
                PersonNotifyContextGet,
                &[&"templates", &template, &recipient.id()],
            )
            .token(&selected)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(response, ordinary);
    let wrong_kind = key(
        &s.world,
        s.owner.id(),
        vec![rule("PersonNotifyContextGet", "host", template, "host")],
    )
    .await;
    checked(
        s.world
            .app
            .call(
                PersonNotifyContextGet,
                &[&"templates", &template, &recipient.id()],
            )
            .token(&wrong_kind)
            .send_bytes()
            .await,
        StatusCode::FORBIDDEN,
    );
}

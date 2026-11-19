//! Native holder candidates keep exact identity scope, ordinary visibility and current addresses.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

mod places;
mod support;

use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::access::{HolderSearch, HolderSearchQuery};
use orch_api_contract::directory::{OrgGroupCreate, OrgGroupCreateBody};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;

fn rule(operation: &str, action: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],
        "allow":[{"operation":operation,"actions":[action]}]})
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
            name: "Holder selection fixture",
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

fn checked(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

async fn search(world: &World, secret: &str, query: &HolderSearchQuery) -> RawReply {
    world
        .app
        .call(HolderSearch, &[])
        .token(secret)
        .query(query)
        .send_bytes()
        .await
}

async fn group(world: &World, owner: &places::Who, org: Id, name: &str) -> Id {
    world
        .app
        .call(OrgGroupCreate, &[&org])
        .token(&owner.token)
        .body(&OrgGroupCreateBody {
            name: name.into(),
            description: Some("Native group description".into()),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("ordinary group creation keeps its full body")
        .id
}

#[tokio::test]
async fn holder_places_use_exact_operation_action_and_current_typed_identity() {
    let world = World::new().await;
    let owner = places::person(&world, "holder-owner").await;
    let org = places::make_org(&world, &owner, "Private root name", "holder-root")
        .await
        .base
        .id;
    let project = places::make_project(&world, &owner, org, "Selected work")
        .await
        .base
        .id;
    let other = places::make_project(&world, &owner, org, "Other work")
        .await
        .base
        .id;
    let query = HolderSearchQuery {
        place_id: Some(project),
        org_id: Some(org),
        list_places: Some(true),
        ..Default::default()
    };
    let own = key(
        &world,
        owner.id(),
        vec![rule("HolderSearch", "read", "project", project)],
    )
    .await;
    let reply = checked(search(&world, &own, &query).await);
    assert_eq!(reply["items"].as_array().unwrap().len(), 1);
    assert_eq!(reply["items"][0]["input"]["at"], json!(project));
    assert!(reply.to_string().contains("Selected work"));
    assert!(!reply.to_string().contains("Private root name"));
    assert!(!reply.to_string().contains("Other work"));
    for rules in [
        vec![rule("HolderSearch", "manage", "project", project)],
        vec![rule("NodeTagList", "read", "project", project)],
    ] {
        let denied = key(&world, owner.id(), rules).await;
        let response = search(&world, &denied, &query).await;
        assert_eq!(response.status, StatusCode::FORBIDDEN);
        assert!(!String::from_utf8_lossy(&response.raw).contains("Selected work"));
    }
    let sibling = key(
        &world,
        owner.id(),
        vec![rule("HolderSearch", "read", "project", other)],
    )
    .await;
    let filtered = checked(
        search(
            &world,
            &sibling,
            &HolderSearchQuery {
                q: "selected-work".into(),
                ..query.clone()
            },
        )
        .await,
    );
    assert_eq!(filtered["items"], json!([]));
    let empty = checked(
        search(
            &world,
            &own,
            &HolderSearchQuery {
                q: "no-holder-matches-this".into(),
                ..query
            },
        )
        .await,
    );
    assert_eq!(empty["items"], json!([]));
}

#[tokio::test]
async fn holder_group_content_and_address_names_use_the_same_operation_without_lending_visibility()
{
    let world = World::new().await;
    let owner = places::person(&world, "holder-owner").await;
    let stranger = places::person(&world, "holder-stranger").await;
    let org = places::make_org(&world, &owner, "Keeper name sentinel", "holder-keepers")
        .await
        .base
        .id;
    let group = group(&world, &owner, org, "holder-team").await;
    let query = HolderSearchQuery {
        q: "holder-team".into(),
        org_id: Some(org),
        place_id: Some(org),
        ..Default::default()
    };
    let limited = key(
        &world,
        owner.id(),
        vec![
            rule("HolderSearch", "read", "group", group),
            rule("NodeTagList", "read", "org", org),
        ],
    )
    .await;
    let reply = checked(search(&world, &limited, &query).await);
    assert_eq!(reply["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        reply["items"][0]["input"],
        json!({"kind":"group","id":group})
    );
    assert_eq!(
        reply["items"][0]["holder"]["group"]["description"],
        "Native group description"
    );
    assert!(!reply.to_string().contains("Keeper name sentinel"));
    let full = key(
        &world,
        owner.id(),
        vec![
            rule("HolderSearch", "read", "group", group),
            rule("HolderSearch", "read", "org", org),
        ],
    )
    .await;
    let scoped = checked(search(&world, &full, &query).await);
    let ordinary = checked(search(&world, &owner.token, &query).await);
    assert_eq!(scoped, ordinary);
    assert!(scoped.to_string().contains("Keeper name sentinel"));
    let no_acl = key(
        &world,
        stranger.id(),
        vec![rule("HolderSearch", "read", "group", group)],
    )
    .await;
    assert_eq!(
        checked(search(&world, &no_acl, &query).await)["items"],
        json!([])
    );
}

#[tokio::test]
async fn holder_account_scope_filters_before_thirty_and_membership_is_not_account_ancestry() {
    let world = World::new().await;
    let owner = places::person(&world, "holder-owner").await;
    let org = places::make_org(
        &world,
        &owner,
        "Directory root sentinel",
        "holder-directory",
    )
    .await
    .base
    .id;
    let mut people = Vec::new();
    // The existing native holder window is thirty; the last allowed account is beyond it.
    for index in 0..31 {
        let person = places::person(&world, &format!("holder-window-{index:02}")).await;
        places::join(&world, org, org, &person, "viewer", &owner).await;
        people.push(person);
    }
    let last = people.last().unwrap().id();
    let exact = key(
        &world,
        owner.id(),
        vec![rule("HolderSearch", "read", "account", last)],
    )
    .await;
    let query = HolderSearchQuery {
        q: "holder-window".into(),
        org_id: Some(org),
        place_id: Some(org),
        ..Default::default()
    };
    let reply = checked(search(&world, &exact, &query).await);
    assert_eq!(reply["items"].as_array().unwrap().len(), 1);
    assert_eq!(reply["items"][0]["input"], json!({"kind":"user","id":last}));
    assert_eq!(reply["items"][0]["locator"], "@holder-window-30");
    assert!(!reply.to_string().contains("Directory root sentinel"));
    let bounded_to_org = key(
        &world,
        owner.id(),
        vec![json!({
            "target":{"kind":"org","id":org,"reach":"subtree"},"resources":["account"],
            "allow":[{"operation":"HolderSearch","actions":["read"]}]
        })],
    )
    .await;
    assert_eq!(
        checked(search(&world, &bounded_to_org, &query).await)["items"],
        json!([])
    );
    let first = key(
        &world,
        owner.id(),
        vec![rule("HolderSearch", "read", "account", people[0].id())],
    )
    .await;
    let exact_query = HolderSearchQuery {
        q: "@holder-window-30".into(),
        ..query
    };
    assert_eq!(
        checked(search(&world, &first, &exact_query).await)["items"],
        json!([])
    );
}

#[tokio::test]
async fn holder_publication_keeps_the_original_group_keeper_association_after_activity_wait() {
    for change in [false, true] {
        let world = World::new().await;
        let owner = places::person(&world, "holder-owner").await;
        let first = places::make_org(&world, &owner, "First keeper", "holder-first")
            .await
            .base
            .id;
        let second = places::make_org(&world, &owner, "Second keeper", "holder-second")
            .await
            .base
            .id;
        let group = group(&world, &owner, first, "held-holder-team").await;
        let secret = key(
            &world,
            owner.id(),
            vec![rule("HolderSearch", "read", "group", group)],
        )
        .await;
        let query = HolderSearchQuery {
            q: "held-holder-team".into(),
            ..Default::default()
        };
        let before = checked(search(&world, &secret, &query).await);
        assert_eq!(
            before["items"][0]["holder"]["group"]["org_id"],
            json!(first)
        );
        let mut lock = world.db.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *lock)
            .await
            .unwrap();
        let app = world.app.clone();
        let held_key = secret.clone();
        let held_query = query.clone();
        let request = tokio::spawn(async move {
            let call = app
                .call(HolderSearch, &[])
                .token(&held_key)
                .query(&held_query);
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!request.is_finished(), "the actual activity wait was not reached");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(blocker).fetch_one(&world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the native holder response must wait at its activity INSERT");
        if change {
            let mut conn = world.conn().await;
            sqlx::query("UPDATE groups SET org_id=$2 WHERE id=$1")
                .bind(group)
                .bind(second)
                .execute(&mut *conn)
                .await
                .unwrap();
            orch_authz::epoch::bump(&mut conn).await.unwrap();
        }
        lock.rollback().await.unwrap();
        let reply = request.await.unwrap();
        if change {
            assert_eq!(reply.status, StatusCode::FORBIDDEN);
            assert!(!String::from_utf8_lossy(&reply.raw).contains("held-holder-team"));
            let current = checked(search(&world, &secret, &query).await);
            assert_eq!(
                current["items"][0]["holder"]["group"]["org_id"],
                json!(second)
            );
        } else {
            assert_eq!(checked(reply), before);
        }
    }
}

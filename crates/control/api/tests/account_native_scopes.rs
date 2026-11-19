//! Native account search preserves ranking, scope and ordinary directory visibility.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

#[path = "account_native_scopes/activity.rs"]
mod activity;
#[path = "account_native_scopes/authority.rs"]
mod authority;
#[path = "account_native_scopes/bootstrap.rs"]
mod bootstrap;
mod chat_world;
mod desk;
#[path = "account_native_scopes/first_steps.rs"]
mod first_steps;
mod places;
#[path = "account_native_scopes/profile.rs"]
mod profile;
mod resources;
mod search_world;
#[path = "account_native_scopes/shares.rs"]
mod shares;
mod support;
mod usage_world;

use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::directory::{OrgServiceAccountCreate, OrgServiceAccountCreateBody};
use orch_api_contract::users::{USER_SEARCH_MAX_RESULTS, UserSearch, UserSearchQuery};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;

fn rule(kind: &str, id: Id, reach: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"resources":["account"],
        "allow":[{"operation":"UserSearch","actions":["read"]}]})
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
            name: "Native account search fixture",
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

async fn wait_for_activity(
    world: &World,
    blocker: i32,
    request: &tokio::task::JoinHandle<RawReply>,
) {
    // Existing native wait watchdog; only the observed blocked INSERT establishes the boundary.
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            assert!(!request.is_finished(), "the actual activity append must be blocked");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(blocker).fetch_one(&world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the native response publication wait must be observed");
}

#[tokio::test]
async fn account_search_filters_before_its_native_limit_and_keeps_exact_first_keysets() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let org = places::make_org(&world, &owner, "Search root", "search-root")
        .await
        .base
        .id;
    let mut people = Vec::new();
    // One more than the native limit proves this cannot be a post-limit DTO filter.
    for index in 0..=USER_SEARCH_MAX_RESULTS {
        let person = places::person(&world, &format!("search-{index:02}")).await;
        places::join(&world, org, org, &person, "viewer", &owner).await;
        sqlx::query("UPDATE users SET display_name=$2 WHERE id=$1")
            .bind(person.id())
            .bind(format!("search-00 matching person {index}"))
            .execute(&world.db.pool)
            .await
            .unwrap();
        people.push(person);
    }
    let last = people.last().unwrap().id();
    let secret = key(&world, owner.id(), vec![rule("account", last, "self")]).await;
    let query = UserSearchQuery {
        q: "search-00".into(),
        org_id: Some(org),
    };
    let response = checked(
        world
            .app
            .call(UserSearch, &[])
            .token(&secret)
            .query(&query)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(response.as_array().unwrap().len(), 1);
    assert_eq!(response[0]["id"], json!(last));
    let mut conn = world.conn().await;
    let first = identity::search_user_rows(&mut conn, owner.id(), "search-00", Some(org), 1, None)
        .await
        .unwrap();
    assert_eq!(first[0].user.username, "search-00");
    let next = identity::search_user_rows(
        &mut conn,
        owner.id(),
        "search-00",
        Some(org),
        1,
        Some((&first[0].user.username, first[0].user.id)),
    )
    .await
    .unwrap();
    assert_eq!(next[0].user.username, "search-01");
}

#[tokio::test]
async fn account_search_keeps_native_org_view_and_public_exact_match_prerequisites() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let member = places::person(&world, "member").await;
    let stranger = places::person(&world, "stranger").await;
    let org = places::make_org(&world, &owner, "Private search root", "private-search")
        .await
        .base
        .id;
    places::join(&world, org, org, &member, "viewer", &owner).await;
    let secret = key(
        &world,
        stranger.id(),
        vec![rule("account", member.id(), "self")],
    )
    .await;
    let denied = world
        .app
        .call(UserSearch, &[])
        .token(&secret)
        .query(&UserSearchQuery {
            q: "member".into(),
            org_id: Some(org),
        })
        .send_bytes()
        .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&denied.raw).contains("Private search root"));
    let public = checked(
        world
            .app
            .call(UserSearch, &[])
            .token(&secret)
            .query(&UserSearchQuery {
                q: "member".into(),
                org_id: None,
            })
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(public[0]["id"], json!(member.id()));
}

#[tokio::test]
async fn account_search_uses_service_ownership_without_making_human_members_descendants() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let org = places::make_org(&world, &owner, "Account keeping org", "account-keepers")
        .await
        .base
        .id;
    let service = world
        .app
        .call(OrgServiceAccountCreate, &[&org])
        .token(&owner.token)
        .body(&OrgServiceAccountCreateBody {
            username: "search-service".into(),
            display_name: "Service".into(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this ordinary control requires the full native creation response");
    let secret = key(&world, owner.id(), vec![rule("org", org, "subtree")]).await;
    let service_result = checked(
        world
            .app
            .call(UserSearch, &[])
            .token(&secret)
            .query(&UserSearchQuery {
                q: "search-service".into(),
                org_id: None,
            })
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(service_result[0]["id"], json!(service.user.id));
    let human = checked(
        world
            .app
            .call(UserSearch, &[])
            .token(&secret)
            .query(&UserSearchQuery {
                q: "owner".into(),
                org_id: Some(org),
            })
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(human, json!([]));
}

#[tokio::test]
async fn account_search_publication_cannot_borrow_a_new_native_selection_for_its_old_body() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let first = places::person(&world, "candidate-one").await;
    let second = places::person(&world, "candidate-two").await;
    let org = places::make_org(&world, &owner, "Search publication", "search-publication")
        .await
        .base
        .id;
    for person in [&first, &second] {
        places::join(&world, org, org, person, "viewer", &owner).await;
    }
    sqlx::query("UPDATE users SET display_name=$2 WHERE id=$1")
        .bind(first.id())
        .bind("Match private earlier display")
        .execute(&world.db.pool)
        .await
        .unwrap();
    let secret = key(
        &world,
        owner.id(),
        vec![
            rule("account", first.id(), "self"),
            rule("account", second.id(), "self"),
        ],
    )
    .await;
    let query = UserSearchQuery {
        q: "match".into(),
        org_id: Some(org),
    };
    let control = checked(
        world
            .app
            .call(UserSearch, &[])
            .token(&secret)
            .query(&query)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(control[0]["id"], json!(first.id()));
    let mut blocker = world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let request = tokio::spawn(async move {
        let call = app.call(UserSearch, &[]).token(&secret).query(&query);
        call.send_bytes().await
    });
    // The established native database-wait watchdog; an observed real INSERT is the premise.
    let wait = std::time::Duration::from_secs(30);
    tokio::time::timeout(wait, async {
        loop {
            assert!(!request.is_finished(), "activity wait was not reached");
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                 AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid))
                 AND query LIKE '%INSERT INTO api_token_activity%')",
            )
            .bind(pid)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
            if blocked {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the actual completed response must wait at activity append");
    sqlx::query("DELETE FROM org_members WHERE org_id=$1 AND user_id=$2")
        .bind(org)
        .bind(first.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET display_name=$2 WHERE id=$1")
        .bind(second.id())
        .bind("Match new native result")
        .execute(&world.db.pool)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Match private earlier display"));
    let native = identity::search_user_rows(
        &mut *world.conn().await,
        owner.id(),
        "match",
        Some(org),
        i64::from(USER_SEARCH_MAX_RESULTS),
        None,
    )
    .await
    .unwrap();
    assert_eq!(native.len(), 1);
    assert_eq!(native[0].user.id, second.id());
}

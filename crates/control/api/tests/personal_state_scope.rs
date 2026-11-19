//! Personal-state scopes retain the acting account and current guarded authority.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use axum::body::{Body, to_bytes};
use chrono::Duration;
use http::{Request, StatusCode};
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::{ErrorCode, MAX_JSON_BYTES};
use orch_api_contract::nodes::NodeGet;
use orch_api_contract::personal::{MeProjectViewDelete, MeStateGet, MeStateSet};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::registry::Registry;
use orch_db::queries::{identity, transaction_guard};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::time::Duration as Wait;
use support::World;
use tower::ServiceExt;

// This is scheduling headroom on the shared test machine. Database lock state or
// real request completion decides the rendezvous; no elapsed sleep grants authority.
const LOCK_WAIT: Wait = Wait::from_secs(30);

struct Key {
    id: Id,
    secret: String,
}

async fn key(world: &World, user: Id, target: Id, operations: &[(&str, &str)]) -> Key {
    let allow = operations
        .iter()
        .map(|(operation, action)| {
            json!({
                "operation":operation,"actions":[action]
            })
        })
        .collect::<Vec<_>>();
    let scope: KeyScope = serde_json::from_value(json!({"rules":[{
        "target":{"kind":"account","id":target,"reach":"self"},
        "resources":["account"],"allow":allow
    }]}))
    .unwrap();
    assert!(scope.validate().is_empty());
    let scope = serde_json::to_value(scope).unwrap();
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let mut conn = world.conn().await;
    identity::create_api_token_scoped(
        &mut conn,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Personal state control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: world.now(),
            // A one-hour fixture fits existing place ceilings without moving the clock.
            expires_at: Some(world.now() + Duration::hours(1)),
        },
        Some(&scope),
    )
    .await
    .unwrap();
    Key { id, secret }
}

async fn seed(world: &World, user: Id, name: &str, value: &Json) {
    let mut conn = world.conn().await;
    identity::put_interface_state(&mut conn, user, name, value, world.now())
        .await
        .unwrap();
}

async fn stored(world: &World, user: Id, name: &str) -> Json {
    let mut conn = world.conn().await;
    identity::interface_state(&mut conn, user)
        .await
        .unwrap()
        .into_iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
        .unwrap_or(Json::Null)
}

fn saved_views(entries: &[(Id, &str)]) -> Json {
    Json::Object(
        entries
            .iter()
            .map(|(id, label)| (id.to_string(), json!({"remembered":label})))
            .collect(),
    )
}

#[tokio::test]
async fn state_read_and_write_are_independent_account_scopes_with_empty_write_receipts() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let other = places::person(&world, "other").await;
    let before = json!({"panel":"old"});
    let foreign = json!({"panel":"other account"});
    seed(&world, owner.id(), "sidebar", &before).await;
    seed(&world, other.id(), "sidebar", &foreign).await;
    let read = key(&world, owner.id(), owner.id(), &[("MeStateGet", "read")]).await;
    let write = key(&world, owner.id(), owner.id(), &[("MeStateSet", "write")]).await;
    let wrong_account = key(
        &world,
        owner.id(),
        other.id(),
        &[("MeStateGet", "read"), ("MeStateSet", "write")],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(MeStateGet, &[])
            .token(&read.secret)
            .send()
            .await
            .ok()["sidebar"],
        before
    );
    let changed = json!({"panel":"new","compact":true});
    let reply = world
        .app
        .call(MeStateSet, &[&"sidebar"])
        .token(&write.secret)
        .body(&changed)
        .send()
        .await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert!(reply.raw.is_empty());
    assert_eq!(
        world
            .app
            .call(MeStateGet, &[])
            .token(&write.secret)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN,
        "a write grant does not provide readback"
    );
    assert_eq!(
        world
            .app
            .call(MeStateSet, &[&"sidebar"])
            .token(&read.secret)
            .body(&before)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        world
            .app
            .call(MeStateGet, &[])
            .token(&wrong_account.secret)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        world
            .app
            .call(MeStateSet, &[&"sidebar"])
            .token(&wrong_account.secret)
            .body(&before)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    // An inert/retired action selection cannot be upgraded to the required Write.
    let wrong_action = key(&world, owner.id(), owner.id(), &[("MeStateSet", "read")]).await;
    assert_eq!(
        world
            .app
            .call(MeStateSet, &[&"sidebar"])
            .token(&wrong_action.secret)
            .body(&before)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        world
            .app
            .call(MeStateGet, &[])
            .token(&read.secret)
            .send()
            .await
            .ok()["sidebar"],
        changed
    );
    assert_eq!(stored(&world, other.id(), "sidebar").await, foreign);
}

#[tokio::test]
async fn clearing_saved_project_views_needs_only_own_account_write_not_project_authority() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let other = places::person(&world, "other").await;
    let org = places::make_org(&world, &other, "Other office", "other-office")
        .await
        .base
        .id;
    let private = places::make_project(&world, &other, org, "Private project")
        .await
        .base
        .id;
    let missing = new_id();
    let kept = new_id();
    let unseen = world
        .app
        .call(NodeGet, &[&private])
        .token(&owner.token)
        .send()
        .await;
    assert!(
        matches!(unseen.status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "the fixture caller cannot read the existing project"
    );
    let original = saved_views(&[
        (private, "old private view"),
        (missing, "gone project"),
        (kept, "keep"),
    ]);
    let foreign = saved_views(&[
        (private, "other account's view"),
        (missing, "other stale view"),
    ]);
    seed(&world, owner.id(), "project_views", &original).await;
    seed(&world, owner.id(), "sidebar", &json!({"unchanged":true})).await;
    seed(&world, other.id(), "project_views", &foreign).await;
    let wrong = key(
        &world,
        owner.id(),
        other.id(),
        &[("MeProjectViewDelete", "write")],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(MeProjectViewDelete, &[&private])
            .token(&wrong.secret)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(stored(&world, owner.id(), "project_views").await, original);
    let write = key(
        &world,
        owner.id(),
        owner.id(),
        &[("MeProjectViewDelete", "write")],
    )
    .await;
    for project in [private, missing] {
        let removed = world
            .app
            .call(MeProjectViewDelete, &[&project])
            .token(&write.secret)
            .send()
            .await;
        assert_eq!(removed.status, StatusCode::NO_CONTENT);
        assert!(removed.raw.is_empty());
    }
    assert_eq!(
        stored(&world, owner.id(), "project_views").await,
        saved_views(&[(kept, "keep")])
    );
    assert_eq!(
        stored(&world, owner.id(), "sidebar").await,
        json!({"unchanged":true})
    );
    assert_eq!(stored(&world, other.id(), "project_views").await, foreign);
}

fn router(world: &World) -> axum::Router {
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    registry
        .into_router_for(orch_api_contract::surface::HttpSurface::Key)
        .with_state(world.app.app.clone())
}

#[derive(Debug)]
enum Rendezvous {
    Completed,
    Blocked(Vec<String>),
}

async fn rendezvous(
    world: &World,
    blocker: i32,
    action: &tokio::task::JoinHandle<axum::response::Response>,
) -> Rendezvous {
    tokio::time::timeout(LOCK_WAIT, async {
        loop {
            if action.is_finished() {
                return Rendezvous::Completed;
            }
            let queries: Vec<String> = sqlx::query_scalar(
                "SELECT query FROM pg_stat_activity
                  WHERE datname=current_database() AND wait_event_type='Lock'
                    AND $1=ANY(pg_blocking_pids(pid)) ORDER BY pid",
            )
            .bind(blocker)
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
            if !queries.is_empty() {
                return Rendezvous::Blocked(queries);
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the real router request either waits on the held identity or completes")
}

async fn freshness_case(operation: &str, method: &str, action_name: &str, narrow: bool) {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let project = new_id();
    let (state_key, before, expected, path, body) = match operation {
        "MeProjectViewDelete" => (
            "project_views",
            saved_views(&[(project, "saved")]),
            json!({}),
            format!("/me/state/project_views/{project}"),
            Body::empty(),
        ),
        "MeStateSet" => (
            "sidebar",
            json!({"panel":"old"}),
            json!({"panel":"new"}),
            "/me/state/sidebar".to_owned(),
            Body::from(serde_json::to_vec(&json!({"panel":"new"})).unwrap()),
        ),
        "MeStateGet" => (
            "sidebar",
            json!({"panel":"old"}),
            json!({"panel":"old"}),
            "/me/state".to_owned(),
            Body::empty(),
        ),
        _ => unreachable!("the tested endpoint set is explicit"),
    };
    seed(&world, owner.id(), state_key, &before).await;
    let writer = key(&world, owner.id(), owner.id(), &[(operation, action_name)]).await;
    let mut blocker = world.db.pool.begin().await.unwrap();
    transaction_guard::lock_person(&mut blocker, owner.id())
        .await
        .unwrap()
        .unwrap();
    // A real credential mutation owns its key row too. Authentication's existing
    // SKIP LOCKED read fallback remains usable while this row is pinned; a late
    // recorder on an unguarded baseline cannot create a user/key lock cycle.
    sqlx::query("SELECT id FROM api_tokens WHERE id=$1 AND user_id=$2 FOR UPDATE")
        .bind(writer.id)
        .bind(owner.id())
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {}", writer.secret))
        .header("content-type", "application/json")
        .body(body)
        .unwrap();
    let app = router(&world);
    let action = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    let observed = rendezvous(&world, pid, &action).await;
    if narrow {
        let denied = serde_json::to_value(KeyScope::default()).unwrap();
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1 AND user_id=$3")
            .bind(writer.id)
            .bind(denied)
            .bind(owner.id())
            .execute(&mut *blocker)
            .await
            .unwrap();
    }
    // Always release the fixture blocker before checking the outcome, including
    // the old implementation's early completion or post-write recorder wait.
    blocker.commit().await.unwrap();
    let response = tokio::time::timeout(LOCK_WAIT, action)
        .await
        .expect("the released real request finishes")
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), MAX_JSON_BYTES)
        .await
        .unwrap();
    let after = stored(&world, owner.id(), state_key).await;
    let diagnostic = match &observed {
        Rendezvous::Completed => "early completion".to_owned(),
        Rendezvous::Blocked(queries) => format!("observed database wait: {queries:?}"),
    };
    if narrow {
        assert_eq!(status, StatusCode::FORBIDDEN, "{operation}: {diagnostic}");
        let problem: orch_api_contract::common::Problem = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(problem.code, ErrorCode::AuthzDenied);
        assert_eq!(
            after, before,
            "{operation} changed state after its scope was narrowed: {diagnostic}"
        );
    } else if operation == "MeStateGet" {
        assert_eq!(status, StatusCode::OK, "{operation}: {diagnostic}");
        let state: Json = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(state[state_key], before);
        assert_eq!(after, expected);
    } else {
        assert_eq!(status, StatusCode::NO_CONTENT, "{operation}: {diagnostic}");
        assert!(bytes.is_empty());
        assert_eq!(after, expected);
    }
}

#[tokio::test]
async fn project_view_delete_rechecks_rules_after_its_actual_identity_lock_wait() {
    freshness_case("MeProjectViewDelete", "DELETE", "write", false).await;
    freshness_case("MeProjectViewDelete", "DELETE", "write", true).await;
}

#[tokio::test]
async fn state_get_rechecks_rules_before_its_actual_read() {
    freshness_case("MeStateGet", "GET", "read", false).await;
    freshness_case("MeStateGet", "GET", "read", true).await;
}

#[tokio::test]
async fn existing_state_writer_rechecks_rules_at_its_action_guard() {
    freshness_case("MeStateSet", "PUT", "write", true).await;
}

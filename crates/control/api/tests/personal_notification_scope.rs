//! Account notification preferences and removals retain current scope and ordinary ownership.
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
use orch_api_contract::notifications::{
    MeNotificationPrefsGet, MeNotificationRouteDelete, MeNotificationRouteRemove,
    MePushSubscriptionDelete, MePushSubscriptionDeleteBody, MePushSubscriptionList,
    MePushSubscriptionRemove,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::registry::Registry;
use orch_api_core::testing::Reply;
use orch_db::queries::notifications::{channels, inbox, outbox, pages, people, routes};
use orch_db::queries::{identity, settings, transaction_guard};
use orch_types::{Id, new_id};
use orch_worker_core::bot_links;
use serde_json::json;
use std::time::Duration as Wait;
use support::World;
use tower::ServiceExt;

// Scheduling headroom only: actual DB blocking or request completion decides the rendezvous.
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
    stored_key(world, user, Some(serde_json::to_value(scope).unwrap())).await
}

async fn stored_key(world: &World, user: Id, scope: Option<serde_json::Value>) -> Key {
    let scopes = if scope.is_some() {
        Vec::new()
    } else {
        vec!["*".to_owned()]
    };
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let mut conn = world.conn().await;
    identity::create_api_token_scoped(
        &mut conn,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Personal notification control",
            token_hash: &hash,
            scopes: &scopes,
            node_id: None,
            at: world.now(),
            // A one-hour fixture fits existing place ceilings without moving the clock.
            expires_at: Some(world.now() + Duration::hours(1)),
        },
        scope.as_ref(),
    )
    .await
    .unwrap();
    Key { id, secret }
}

async fn push(world: &World, user: Id, name: &str) -> (Id, String) {
    let id = new_id();
    let endpoint = format!("https://push.example.invalid/{name}/{id}");
    let mut conn = world.conn().await;
    people::put_push_subscription(
        &mut conn,
        &people::NewPushSubscription {
            id,
            user_id: user,
            endpoint: &endpoint,
            p256dh: "private-push-key",
            auth: "private-push-auth",
            user_agent: "Firefox",
            at: world.now(),
        },
    )
    .await
    .unwrap();
    (id, endpoint)
}

async fn device_ids(world: &World, user: Id) -> Vec<Id> {
    let mut conn = world.conn().await;
    pages::push_devices_of(&mut conn, user)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.id)
        .collect()
}

async fn channel(world: &World, owner: Id, personal: bool) -> Id {
    let id = new_id();
    let mut conn = world.conn().await;
    channels::insert_channel(
        &mut conn,
        &channels::NewChannel {
            id,
            org_id: None,
            user_id: personal.then_some(owner),
            kind: "webhook",
            name: "Private channel label",
            config: &json!({}),
            templates: &json!({}),
            enabled: true,
            created_by: owner,
            at: world.now(),
        },
    )
    .await
    .unwrap();
    id
}

async fn linked_route(world: &World, user: Id, channel: Id) -> routes::RouteRow {
    let id = new_id();
    let mut conn = world.conn().await;
    routes::put_route(
        &mut conn,
        id,
        user,
        channel,
        &routes::RouteChange {
            enabled: Some(true),
            ..Default::default()
        },
        world.now(),
    )
    .await
    .unwrap();
    let hash = token_hash(&id.to_string());
    routes::start_link(
        &mut conn,
        id,
        &hash,
        world.now() + Duration::minutes(10),
        world.now(),
    )
    .await
    .unwrap();
    assert!(
        routes::complete_link(
            &mut conn,
            id,
            &hash,
            &json!({"address":"private destination"}),
            world.now()
        )
        .await
        .unwrap()
    );
    routes::route(&mut conn, id).await.unwrap().unwrap()
}

async fn pending(world: &World, route: Id) -> Id {
    let id = new_id();
    let mut conn = world.conn().await;
    outbox::insert(
        &mut conn,
        &outbox::NewRow {
            id,
            channel: "webhook",
            route_id: Some(route),
            destination_id: None,
            channel_id: None,
            payload: &outbox::Payload::default().to_json(),
            at: world.now(),
        },
    )
    .await
    .unwrap();
    id
}

fn empty(reply: Reply<()>) {
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert!(
        reply.raw.is_empty(),
        "Manage-only removals are content-free"
    );
    reply.ok();
}

#[tokio::test]
async fn personal_reads_require_the_current_account_and_keep_device_secrets_hidden() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let other = places::person(&world, "other").await;
    let own_device = push(&world, owner.id(), "own").await;
    let foreign_device = push(&world, other.id(), "foreign").await;
    let mut conn = world.conn().await;
    inbox::put_prefs(
        &mut conn,
        owner.id(),
        &inbox::PrefsChange {
            approvals_always: Some(false),
            quiet: Some(Some((60, 120))),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    inbox::put_prefs(
        &mut conn,
        other.id(),
        &inbox::PrefsChange {
            approvals_always: Some(true),
            quiet: Some(Some((180, 240))),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    drop(conn);
    let operations = [
        ("MeNotificationPrefsGet", "read"),
        ("MePushSubscriptionList", "read"),
    ];
    let read = key(&world, owner.id(), owner.id(), &operations).await;
    let legacy = stored_key(&world, owner.id(), None).await;
    let wrong_account = key(&world, owner.id(), other.id(), &operations).await;
    let wrong_action = key(
        &world,
        owner.id(),
        owner.id(),
        &[
            ("MeNotificationPrefsGet", "manage"),
            ("MePushSubscriptionList", "manage"),
        ],
    )
    .await;
    let expected = world
        .app
        .call(MeNotificationPrefsGet, &[])
        .token(&owner.token)
        .send()
        .await
        .ok();
    assert!(!expected.approvals_always);
    assert_eq!(expected.quiet_hours.as_ref().unwrap().start_minute, 60);
    assert_eq!(
        world
            .app
            .call(MeNotificationPrefsGet, &[])
            .token(&read.secret)
            .send()
            .await
            .ok(),
        expected
    );
    assert_eq!(
        world
            .app
            .call(MeNotificationPrefsGet, &[])
            .token(&legacy.secret)
            .send()
            .await
            .ok(),
        expected
    );
    let reply = world
        .app
        .call(MePushSubscriptionList, &[])
        .token(&read.secret)
        .send()
        .await;
    let text = String::from_utf8_lossy(&reply.raw);
    for secret in [
        own_device.1.as_str(),
        foreign_device.1.as_str(),
        "private-push-key",
        "private-push-auth",
    ] {
        assert!(
            !text.contains(secret),
            "push list must use its existing safe metadata projection"
        );
    }
    let rows = reply.ok();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, own_device.0);
    assert_eq!(rows[0].endpoint_hash.len(), 64);
    assert!(
        rows[0]
            .endpoint_hash
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    assert_eq!(
        world
            .app
            .call(MePushSubscriptionList, &[])
            .token(&owner.token)
            .send()
            .await
            .ok(),
        rows
    );
    assert_eq!(
        world
            .app
            .call(MePushSubscriptionList, &[])
            .token(&legacy.secret)
            .send()
            .await
            .ok(),
        rows
    );
    for refused in [&wrong_account.secret, &wrong_action.secret] {
        assert_eq!(
            world
                .app
                .call(MeNotificationPrefsGet, &[])
                .token(refused)
                .send()
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied
        );
        assert_eq!(
            world
                .app
                .call(MePushSubscriptionList, &[])
                .token(refused)
                .send()
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied
        );
    }
}

#[tokio::test]
async fn push_removals_need_manage_only_and_keep_the_existing_owner_filters() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let other = places::person(&world, "other").await;
    let first = push(&world, owner.id(), "first").await;
    let second = push(&world, owner.id(), "second").await;
    let foreign = push(&world, other.id(), "foreign").await;
    let operations = [
        ("MePushSubscriptionDelete", "manage"),
        ("MePushSubscriptionRemove", "manage"),
    ];
    let manage = key(&world, owner.id(), owner.id(), &operations).await;
    let wrong_account = key(&world, owner.id(), other.id(), &operations).await;
    let read = key(
        &world,
        owner.id(),
        owner.id(),
        &[
            ("MePushSubscriptionDelete", "read"),
            ("MePushSubscriptionRemove", "read"),
        ],
    )
    .await;
    for refused in [&wrong_account.secret, &read.secret] {
        assert_eq!(
            world
                .app
                .call(MePushSubscriptionDelete, &[])
                .token(refused)
                .body(&MePushSubscriptionDeleteBody {
                    endpoint: first.1.clone()
                })
                .send()
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied
        );
        assert_eq!(
            world
                .app
                .call(MePushSubscriptionRemove, &[&second.0])
                .token(refused)
                .send()
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied
        );
    }
    assert_eq!(device_ids(&world, owner.id()).await.len(), 2);
    // Endpoint removal deliberately hides absence/other ownership, including on repeated calls.
    for endpoint in [
        foreign.1.clone(),
        format!("  {}  ", first.1),
        first.1.clone(),
    ] {
        empty(
            world
                .app
                .call(MePushSubscriptionDelete, &[])
                .token(&manage.secret)
                .body(&MePushSubscriptionDeleteBody { endpoint })
                .send()
                .await,
        );
    }
    assert_eq!(device_ids(&world, owner.id()).await, [second.0]);
    for id in [foreign.0, new_id()] {
        assert_eq!(
            world
                .app
                .call(MePushSubscriptionRemove, &[&id])
                .token(&manage.secret)
                .send()
                .await
                .status,
            StatusCode::NOT_FOUND
        );
    }
    empty(
        world
            .app
            .call(MePushSubscriptionRemove, &[&second.0])
            .token(&manage.secret)
            .send()
            .await,
    );
    assert!(device_ids(&world, owner.id()).await.is_empty());
    assert_eq!(device_ids(&world, other.id()).await, [foreign.0]);
    assert_eq!(
        world
            .app
            .call(MePushSubscriptionList, &[])
            .token(&manage.secret)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn route_removals_keep_own_off_unlink_and_cleanup_semantics_without_channel_scope() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let other = places::person(&world, "other").await;
    let shared = channel(&world, owner.id(), false).await;
    let private = channel(&world, other.id(), true).await;
    let own = linked_route(&world, owner.id(), shared).await;
    let foreign = linked_route(&world, other.id(), shared).await;
    let private_route = linked_route(&world, other.id(), private).await;
    let own_pending = pending(&world, own.id).await;
    let foreign_pending = pending(&world, foreign.id).await;
    let mut conn = world.conn().await;
    for row in [&own, &foreign] {
        settings::put_secret(
            &mut conn,
            channels::owner::CHANNEL,
            shared,
            &bot_links::link_secret_name(row.id),
            b"sealed link fixture",
            Some(row.user_id),
            world.now(),
        )
        .await
        .unwrap();
    }
    drop(conn);
    let operations = [
        ("MeNotificationRouteDelete", "manage"),
        ("MeNotificationRouteRemove", "manage"),
    ];
    let manage = key(&world, owner.id(), owner.id(), &operations).await;
    let wrong_account = key(&world, owner.id(), other.id(), &operations).await;
    let read = key(
        &world,
        owner.id(),
        owner.id(),
        &[
            ("MeNotificationRouteDelete", "read"),
            ("MeNotificationRouteRemove", "read"),
        ],
    )
    .await;
    for refused in [&wrong_account.secret, &read.secret] {
        assert_eq!(
            world
                .app
                .call(MeNotificationRouteDelete, &[&shared])
                .token(refused)
                .send()
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied
        );
        assert_eq!(
            world
                .app
                .call(MeNotificationRouteRemove, &[&own.id])
                .token(refused)
                .send()
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied
        );
    }
    let on = world
        .app
        .call(MeNotificationRouteRemove, &[&own.id])
        .token(&manage.secret)
        .send()
        .await;
    assert_eq!(on.status, StatusCode::BAD_REQUEST);
    let raw = String::from_utf8_lossy(&on.raw);
    assert!(!raw.contains("Private channel label"));
    assert!(!raw.contains("private destination"));
    let ordinary = world
        .app
        .call(MeNotificationRouteRemove, &[&own.id])
        .token(&owner.token)
        .send()
        .await;
    let mut scoped_problem = on.problem();
    let mut ordinary_problem = ordinary.problem();
    scoped_problem.request_id.clear();
    ordinary_problem.request_id.clear();
    assert_eq!(
        scoped_problem, ordinary_problem,
        "same off-only refusal for browser and key"
    );
    for id in [private, new_id()] {
        assert_eq!(
            world
                .app
                .call(MeNotificationRouteDelete, &[&id])
                .token(&manage.secret)
                .send()
                .await
                .status,
            StatusCode::NOT_FOUND
        );
    }
    for id in [foreign.id, new_id()] {
        assert_eq!(
            world
                .app
                .call(MeNotificationRouteRemove, &[&id])
                .token(&manage.secret)
                .send()
                .await
                .status,
            StatusCode::NOT_FOUND
        );
    }
    // Own Account Manage may end an offered system channel without controlling that channel.
    for _ in 0..2 {
        empty(
            world
                .app
                .call(MeNotificationRouteDelete, &[&shared])
                .token(&manage.secret)
                .send()
                .await,
        );
    }
    let mut conn = world.conn().await;
    let off = routes::route(&mut conn, own.id).await.unwrap().unwrap();
    assert!(!off.enabled);
    assert_eq!(off.address, json!({}));
    assert!(off.linked_at.is_none() && off.link_hash.is_none() && off.link_expires_at.is_none());
    assert_eq!(
        routes::route(&mut conn, foreign.id).await.unwrap(),
        Some(foreign.clone())
    );
    assert_eq!(
        routes::route(&mut conn, private_route.id).await.unwrap(),
        Some(private_route)
    );
    assert_eq!(
        outbox::pending_of_route(&mut conn, own.id).await.unwrap()[0].id,
        own_pending
    );
    drop(conn);
    empty(
        world
            .app
            .call(MeNotificationRouteRemove, &[&own.id])
            .token(&manage.secret)
            .send()
            .await,
    );
    let mut conn = world.conn().await;
    assert!(routes::route(&mut conn, own.id).await.unwrap().is_none());
    assert!(
        settings::secret(
            &mut conn,
            channels::owner::CHANNEL,
            shared,
            &bot_links::link_secret_name(own.id)
        )
        .await
        .unwrap()
        .is_none()
    );
    assert!(
        settings::secret(
            &mut conn,
            channels::owner::CHANNEL,
            shared,
            &bot_links::link_secret_name(foreign.id)
        )
        .await
        .unwrap()
        .is_some()
    );
    // The existing writer cancels pending deliveries before deleting the route;
    // the route FK then cascades its outbox rows. Other routes' work remains.
    let own_delivery_remains: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM outbox WHERE id=$1)")
            .bind(own_pending)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert!(!own_delivery_remains);
    assert_eq!(
        outbox::pending_of_route(&mut conn, foreign.id)
            .await
            .unwrap()[0]
            .id,
        foreign_pending
    );
    assert_eq!(
        routes::route(&mut conn, foreign.id).await.unwrap(),
        Some(foreign)
    );
}

#[tokio::test]
async fn ordinary_browser_and_legacy_removals_keep_their_empty_results() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let legacy = stored_key(&world, owner.id(), None).await;
    for token in [&owner.token, &legacy.secret] {
        let first = push(&world, owner.id(), "endpoint").await;
        let second = push(&world, owner.id(), "device").await;
        let channel = channel(&world, owner.id(), true).await;
        let route = linked_route(&world, owner.id(), channel).await;
        empty(
            world
                .app
                .call(MePushSubscriptionDelete, &[])
                .token(token)
                .body(&MePushSubscriptionDeleteBody { endpoint: first.1 })
                .send()
                .await,
        );
        empty(
            world
                .app
                .call(MePushSubscriptionRemove, &[&second.0])
                .token(token)
                .send()
                .await,
        );
        empty(
            world
                .app
                .call(MeNotificationRouteDelete, &[&channel])
                .token(token)
                .send()
                .await,
        );
        empty(
            world
                .app
                .call(MeNotificationRouteRemove, &[&route.id])
                .token(token)
                .send()
                .await,
        );
        assert!(device_ids(&world, owner.id()).await.is_empty());
        let mut conn = world.conn().await;
        assert!(routes::route(&mut conn, route.id).await.unwrap().is_none());
    }
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

#[derive(Clone, Copy)]
enum FreshCase {
    Preferences,
    Devices,
    DeleteDevice,
    EndRoute,
}

impl FreshCase {
    fn operation(self) -> &'static str {
        match self {
            Self::Preferences => "MeNotificationPrefsGet",
            Self::Devices => "MePushSubscriptionList",
            Self::DeleteDevice => "MePushSubscriptionDelete",
            Self::EndRoute => "MeNotificationRouteDelete",
        }
    }
    fn reads(self) -> bool {
        matches!(self, Self::Preferences | Self::Devices)
    }
}

async fn freshness_case(case: FreshCase, narrow: bool) {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let other = places::person(&world, "other").await;
    let device = push(&world, owner.id(), "protected").await;
    let foreign_device = push(&world, other.id(), "foreign").await;
    let channel = channel(&world, owner.id(), false).await;
    let route = linked_route(&world, owner.id(), channel).await;
    let foreign_route = linked_route(&world, other.id(), channel).await;
    let mut conn = world.conn().await;
    let prefs = inbox::put_prefs(
        &mut conn,
        owner.id(),
        &inbox::PrefsChange {
            approvals_always: Some(false),
            quiet: Some(Some((60, 120))),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    drop(conn);
    let operation = case.operation();
    let key = key(
        &world,
        owner.id(),
        owner.id(),
        &[(operation, if case.reads() { "read" } else { "manage" })],
    )
    .await;
    let mut blocker = world.db.pool.begin().await.unwrap();
    transaction_guard::lock_person(&mut blocker, owner.id())
        .await
        .unwrap()
        .unwrap();
    // Pin the real key as a credential edit would. Authentication's SKIP LOCKED
    // fallback remains usable, and the old recorder cannot form a user/key lock cycle.
    sqlx::query("SELECT id FROM api_tokens WHERE id=$1 AND user_id=$2 FOR UPDATE")
        .bind(key.id)
        .bind(owner.id())
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let (path, body) = match case {
        FreshCase::Preferences => ("/me/notification-prefs".to_owned(), Body::empty()),
        FreshCase::Devices => ("/me/push-subscriptions".to_owned(), Body::empty()),
        FreshCase::DeleteDevice => (
            "/me/push-subscriptions".to_owned(),
            Body::from(
                serde_json::to_vec(&MePushSubscriptionDeleteBody { endpoint: device.1 }).unwrap(),
            ),
        ),
        FreshCase::EndRoute => (format!("/me/notification-routes/{channel}"), Body::empty()),
    };
    let request = Request::builder()
        .method(if case.reads() { "GET" } else { "DELETE" })
        .uri(path)
        .header("authorization", format!("Bearer {}", key.secret))
        .header("content-type", "application/json")
        .body(body)
        .unwrap();
    let app = router(&world);
    let action = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    let observed = rendezvous(&world, pid, &action).await;
    if narrow {
        let denied = serde_json::to_value(KeyScope::default()).unwrap();
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1 AND user_id=$3")
            .bind(key.id)
            .bind(denied)
            .bind(owner.id())
            .execute(&mut *blocker)
            .await
            .unwrap();
    }
    // Always release before assertions: the baseline may already have read/written
    // and be waiting only in its recorder. Neither outcome is scope admission.
    blocker.commit().await.unwrap();
    let response = tokio::time::timeout(LOCK_WAIT, action)
        .await
        .expect("released request finishes")
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), MAX_JSON_BYTES)
        .await
        .unwrap();
    let diagnostic = match observed {
        Rendezvous::Completed => "early completion".to_owned(),
        Rendezvous::Blocked(queries) => format!("database wait: {queries:?}"),
    };
    if narrow {
        assert_eq!(status, StatusCode::FORBIDDEN, "{operation}: {diagnostic}");
        let problem: orch_api_contract::common::Problem = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(problem.code, ErrorCode::AuthzDenied);
        let raw = String::from_utf8_lossy(&bytes);
        assert!(!raw.contains("quiet_hours") && !raw.contains("endpoint_hash"));
    } else if case.reads() {
        assert_eq!(status, StatusCode::OK, "{operation}: {diagnostic}");
        match case {
            FreshCase::Preferences => {
                let shown: orch_api_contract::notifications::NotificationPrefs =
                    serde_json::from_slice(&bytes).unwrap();
                assert_eq!(shown.revision, prefs.revision);
                assert!(!shown.approvals_always);
                assert_eq!(shown.quiet_hours.unwrap().start_minute, 60);
            }
            FreshCase::Devices => {
                let shown: Vec<orch_api_contract::notifications::PushDevice> =
                    serde_json::from_slice(&bytes).unwrap();
                assert_eq!(shown.len(), 1);
                assert_eq!(shown[0].id, device.0);
            }
            _ => unreachable!("only read cases parse data"),
        }
    } else {
        assert_eq!(status, StatusCode::NO_CONTENT, "{operation}: {diagnostic}");
        assert!(bytes.is_empty());
    }
    let removed_device = matches!(case, FreshCase::DeleteDevice) && !narrow;
    assert_eq!(
        device_ids(&world, owner.id()).await.contains(&device.0),
        !removed_device
    );
    assert_eq!(device_ids(&world, other.id()).await, [foreign_device.0]);
    let mut conn = world.conn().await;
    assert_eq!(inbox::prefs(&mut conn, owner.id()).await.unwrap(), prefs);
    let current = routes::route(&mut conn, route.id).await.unwrap().unwrap();
    if matches!(case, FreshCase::EndRoute) && !narrow {
        assert!(!current.enabled);
        assert_eq!(current.address, json!({}));
        assert!(current.linked_at.is_none());
    } else {
        assert_eq!(
            current, route,
            "no route effect for denied or unrelated operations"
        );
    }
    assert_eq!(
        routes::route(&mut conn, foreign_route.id).await.unwrap(),
        Some(foreign_route)
    );
}

#[tokio::test]
async fn preference_read_rechecks_scope_on_its_actual_connection() {
    freshness_case(FreshCase::Preferences, false).await;
    freshness_case(FreshCase::Preferences, true).await;
}

#[tokio::test]
async fn device_list_rechecks_scope_on_its_actual_connection() {
    freshness_case(FreshCase::Devices, false).await;
    freshness_case(FreshCase::Devices, true).await;
}

#[tokio::test]
async fn endpoint_deletion_rechecks_scope_before_its_existing_writer() {
    freshness_case(FreshCase::DeleteDevice, false).await;
    freshness_case(FreshCase::DeleteDevice, true).await;
}

#[tokio::test]
async fn route_end_rechecks_scope_before_its_existing_writer() {
    freshness_case(FreshCase::EndRoute, false).await;
    freshness_case(FreshCase::EndRoute, true).await;
}

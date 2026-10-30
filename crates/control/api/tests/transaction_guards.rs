//! A mutation rechecks the caller's permission inside its transaction after reading the request body.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use axum::body::Body;
use bytes::Bytes;
use chrono::Duration;
use futures::stream;
use http::{Request, StatusCode};
use orch_api_contract::auth::{AdminModeSetBody, MeAdminModeSet};
use orch_api_contract::nodes::{Node, NodeUpdate, NodeUpdateBody, Project};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::middleware::requests::ApiError;
use orch_api_core::middleware::{authorization, transaction_guard};
use orch_api_core::registry::{Ctx, Registry};
use orch_db::queries::access::{self, PageKey, testing};
use orch_db::queries::hierarchy::{self, TextChange};
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Clock, Id, Time, new_id};
use places::{Who, join, make_org, make_project, person};
use serde_json::json;
use std::future::Future;
use std::time::{Duration as WaitDuration, Instant};
use support::World;
use tower::ServiceExt;

#[tokio::test]
async fn revoking_manage_while_a_body_arrives_prevents_the_rename() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let writer = person(&world, "writer").await;
    let org = make_org(&world, &owner, "Office", "office").await;
    join(&world, org.base.id, org.base.id, &writer, "viewer", &owner).await;
    let project = make_project(&world, &owner, org.base.id, "Original").await;
    let entry = {
        let mut conn = world.conn().await;
        testing::add_entry(
            &mut conn,
            &PageKey::new("node", project.base.id, "live"),
            "any",
            "project.manage",
            ("user", Some(writer.id()), None),
            None,
            world.now(),
        )
        .await
        .unwrap()
    };
    let pool = world.db.pool.clone();
    let now = world.now();
    let owner_id = owner.id();
    // This future is polled only after the route's initial permission check succeeds.
    let body = Body::from_stream(stream::once(async move {
        let mut conn = pool.acquire().await.unwrap();
        access::remove_entry(&mut conn, entry, owner_id, now)
            .await
            .unwrap();
        access::bump_epoch(&mut conn).await.unwrap();
        Ok::<_, std::io::Error>(Bytes::from(
            serde_json::to_vec(&json!({
                "name": "Renamed", "expected_revision": project.base.revision
            }))
            .unwrap(),
        ))
    }));
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = registry.into_router().with_state(world.app.app.clone());
    let response = router
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/nodes/{}", project.base.id))
                .header("authorization", format!("Bearer {}", writer.token))
                .header("content-type", "application/json")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        places::read(&world, &owner, project.base.id).await,
        orch_api_contract::nodes::Node::Project(project)
    );
}

#[tokio::test]
async fn an_entry_that_ends_while_the_body_arrives_cannot_manage_the_page() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let writer = person(&world, "writer").await;
    let org = make_org(&world, &owner, "Office", "office").await;
    join(&world, org.base.id, org.base.id, &writer, "viewer", &owner).await;
    let project = make_project(&world, &owner, org.base.id, "Original").await;
    {
        let mut conn = world.conn().await;
        testing::add_entry(
            &mut conn,
            &PageKey::new("node", project.base.id, "live"),
            "any",
            "access.manage",
            ("user", Some(writer.id()), None),
            Some(world.now() + Duration::seconds(1)),
            world.now(),
        )
        .await
        .unwrap();
    }
    let pool = world.db.pool.clone();
    let clock = world.app.clock.clone();
    let writer_id = writer.id();
    let project_id = project.base.id;
    // The permission must be current when the page is changed, after the body is read.
    let body = Body::from_stream(stream::once(async move {
        clock.advance(Duration::seconds(2));
        let mut conn = pool.acquire().await.unwrap();
        let principal = orch_authz::Principal::load(
            &mut conn,
            orch_authz::Caller::session(writer_id),
            Time::new(clock.now()),
        )
        .await
        .unwrap();
        let current = orch_authz::nodes::check(
            &mut conn,
            &principal,
            project_id,
            orch_access_contract::permissions::Permission::AccessManage,
            orch_authz::Act::Use,
            orch_authz::Policy::default(),
        )
        .await
        .unwrap();
        assert!(
            !current.is_allowed(),
            "the entry has ended for an ordinary check"
        );
        Ok::<_, std::io::Error>(Bytes::from(
            serde_json::to_vec(&json!({
                "whole": "private"
            }))
            .unwrap(),
        ))
    }));
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = registry.into_router().with_state(world.app.app.clone());
    let response = router
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/nodes/{project_id}/permissions"))
                .header("authorization", format!("Bearer {}", writer.token))
                .header("content-type", "application/json")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "a transaction cannot reuse the permission as it stood before the body arrived"
    );
    let cut_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM permission_rows WHERE object_id = $1 AND NOT inherit",
    )
    .bind(project_id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(cut_rows, 0, "the refused Private change writes no rows");
}

struct Scenario {
    world: World,
    owner: Who,
    writer: Who,
    project: Project,
    entry: Id,
}

impl Scenario {
    async fn new() -> Self {
        let world = World::new().await;
        let owner = person(&world, "owner").await;
        let writer = person(&world, "writer").await;
        let org = make_org(&world, &owner, "Office", "office").await;
        join(&world, org.base.id, org.base.id, &writer, "viewer", &owner).await;
        let project = make_project(&world, &owner, org.base.id, "Original").await;
        let entry = testing::add_entry(
            &mut *world.conn().await,
            &PageKey::new("node", project.base.id, "live"),
            "any",
            "project.manage",
            ("user", Some(writer.id()), None),
            None,
            world.now(),
        )
        .await
        .unwrap();
        Self {
            world,
            owner,
            writer,
            project,
            entry,
        }
    }

    async fn unchanged(&self) {
        let Node::Project(project) =
            places::read(&self.world, &self.owner, self.project.base.id).await
        else {
            panic!("a project")
        };
        assert_eq!(project.base.name, self.project.base.name);
        assert_eq!(project.base.description, self.project.base.description);
        assert_eq!(project.base.color, self.project.base.color);
        assert_eq!(project.base.revision, self.project.base.revision);
    }

    fn rename(&self) -> serde_json::Value {
        json!({"name": "Renamed", "expected_revision": self.project.base.revision})
    }
}

async fn request_after(
    world: &World,
    who: &Who,
    method: &str,
    path: &str,
    value: serde_json::Value,
    change: impl Future<Output = ()> + Send + 'static,
) -> axum::response::Response {
    // Polling the stream starts only after authentication and permission preflight.
    let body = Body::from_stream(stream::once(async move {
        change.await;
        Ok::<_, std::io::Error>(Bytes::from(serde_json::to_vec(&value).unwrap()))
    }));
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    registry
        .into_router()
        .with_state(world.app.app.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {}", who.token))
                .header("content-type", "application/json")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn a_still_entitled_writer_can_rename_after_a_delayed_body() {
    let scenario = Scenario::new().await;
    let clock = scenario.world.app.clock.clone();
    let response = request_after(
        &scenario.world,
        &scenario.writer,
        "PATCH",
        &format!("/nodes/{}", scenario.project.base.id),
        scenario.rename(),
        async move {
            clock.advance(Duration::seconds(2));
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let node = places::read(&scenario.world, &scenario.owner, scenario.project.base.id).await;
    let orch_api_contract::nodes::Node::Project(project) = node else {
        panic!("a project")
    };
    assert_eq!(project.base.name, "Renamed");
}

#[tokio::test]
async fn administration_lost_while_the_body_arrives_cannot_change_root_limits() {
    for revoke in [false, true] {
        let scenario = Scenario::new().await;
        scenario.world.make_admin(&scenario.owner.person).await;
        scenario.world.make_admin(&scenario.writer.person).await;
        let mode = scenario
            .world
            .app
            .call(MeAdminModeSet, &[])
            .token(&scenario.writer.token)
            .body(&AdminModeSetBody { as_admin: true })
            .send()
            .await
            .ok();
        assert!(mode.as_admin && mode.available);
        let root = scenario.project.base.org_id;
        let pool = scenario.world.db.pool.clone();
        let writer = scenario.writer.id();
        let owner = scenario.owner.id();
        let now = scenario.world.now();
        let response = request_after(
            &scenario.world,
            &scenario.writer,
            "PATCH",
            &format!("/admin/orgs/{root}"),
            json!({"closed": true}),
            async move {
                if revoke {
                    let mut tx = pool.begin().await.unwrap();
                    orch_authz::standing::revoke_admin(
                        &mut tx,
                        writer,
                        &orch_authz::standing::Acting {
                            by: orch_db::audit::Actor::User(owner),
                            origin: &orch_db::audit::Origin::default(),
                            now,
                        },
                    )
                    .await
                    .unwrap();
                    tx.commit().await.unwrap();
                }
            },
        )
        .await;
        assert_eq!(
            response.status(),
            if revoke {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::OK
            }
        );
        let closed: bool = sqlx::query_scalar("SELECT closed FROM orgs WHERE node_id = $1")
            .bind(root)
            .fetch_one(&scenario.world.db.pool)
            .await
            .unwrap();
        assert_eq!(
            closed, !revoke,
            "the limit is written only by a current system admin"
        );
    }
}

#[tokio::test]
async fn a_role_that_ends_during_the_body_cannot_authorize_a_rename() {
    let scenario = Scenario::new().await;
    let now = scenario.world.now();
    {
        let mut conn = scenario.world.conn().await;
        access::remove_entry(&mut conn, scenario.entry, scenario.owner.id(), now)
            .await
            .unwrap();
        testing::give_role(
            &mut conn,
            scenario.project.base.id,
            "user",
            scenario.writer.id(),
            "admin",
            scenario.owner.id(),
            Some(now + Duration::seconds(1)),
        )
        .await
        .unwrap();
    }
    let clock = scenario.world.app.clock.clone();
    let response = request_after(
        &scenario.world,
        &scenario.writer,
        "PATCH",
        &format!("/nodes/{}", scenario.project.base.id),
        scenario.rename(),
        async move {
            clock.advance(Duration::seconds(2));
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    scenario.unchanged().await;
}

#[derive(Debug, Clone, Copy)]
enum SessionLoss {
    Revoked,
    Suspended,
    AccessExpired,
    SessionExpired,
}

#[tokio::test]
async fn identity_lost_during_the_body_prevents_every_rename() {
    for loss in [
        SessionLoss::Revoked,
        SessionLoss::Suspended,
        SessionLoss::AccessExpired,
        SessionLoss::SessionExpired,
    ] {
        let scenario = Scenario::new().await;
        let now = scenario.world.now();
        let writer = scenario.writer.id();
        let hash = token_hash(&scenario.writer.token);
        let session =
            identity::session_by_access_token(&mut *scenario.world.conn().await, &hash, now)
                .await
                .unwrap()
                .unwrap()
                .session_id;
        {
            let mut conn = scenario.world.conn().await;
            match loss {
                SessionLoss::AccessExpired => {
                    sqlx::query("UPDATE access_tokens SET expires_at = $2 WHERE token_hash = $1")
                        .bind(&hash)
                        .bind(now + Duration::seconds(1))
                        .execute(&mut *conn)
                        .await
                        .unwrap();
                }
                SessionLoss::SessionExpired => {
                    sqlx::query("UPDATE sessions SET expires_at = $2 WHERE id = $1")
                        .bind(session)
                        .bind(now + Duration::seconds(1))
                        .execute(&mut *conn)
                        .await
                        .unwrap();
                }
                _ => {}
            }
        }
        let pool = scenario.world.db.pool.clone();
        let clock = scenario.world.app.clock.clone();
        let response = request_after(
            &scenario.world,
            &scenario.writer,
            "PATCH",
            &format!("/nodes/{}", scenario.project.base.id),
            scenario.rename(),
            async move {
                let mut tx = pool.begin().await.unwrap();
                match loss {
                    SessionLoss::Revoked => {
                        identity::revoke_session(&mut tx, session, writer, now)
                            .await
                            .unwrap();
                    }
                    SessionLoss::Suspended => {
                        identity::testing::suspend(&mut tx, writer).await.unwrap();
                    }
                    SessionLoss::AccessExpired | SessionLoss::SessionExpired => {
                        clock.advance(Duration::seconds(2));
                    }
                }
                tx.commit().await.unwrap();
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{loss:?}");
        scenario.unchanged().await;
    }
}

#[derive(Debug, Clone, Copy)]
enum TokenLoss {
    Revoked,
    Expired,
    NarrowScopes,
    OutsideSubtree,
}

#[tokio::test]
async fn current_token_revocation_expiry_scopes_and_subtree_bound_the_write() {
    for loss in [
        TokenLoss::Revoked,
        TokenLoss::Expired,
        TokenLoss::NarrowScopes,
        TokenLoss::OutsideSubtree,
    ] {
        let mut scenario = Scenario::new().await;
        let now = scenario.world.now();
        let token = new_id();
        let other = make_project(
            &scenario.world,
            &scenario.owner,
            scenario.project.base.parent_id.unwrap(),
            "Outside",
        )
        .await;
        scenario.writer.token = new_token(API_TOKEN_PREFIX);
        identity::create_api_token(
            &mut *scenario.world.conn().await,
            &NewApiToken {
                id: token,
                user_id: scenario.writer.id(),
                name: "integration",
                token_hash: &token_hash(&scenario.writer.token),
                scopes: &["*".into()],
                node_id: None,
                at: now,
                expires_at: matches!(loss, TokenLoss::Expired)
                    .then_some(now + Duration::seconds(1)),
            },
        )
        .await
        .unwrap();
        let pool = scenario.world.db.pool.clone();
        let clock = scenario.world.app.clock.clone();
        let writer = scenario.writer.id();
        let response = request_after(
            &scenario.world,
            &scenario.writer,
            "PATCH",
            &format!("/nodes/{}", scenario.project.base.id),
            scenario.rename(),
            async move {
                let mut tx = pool.begin().await.unwrap();
                match loss {
                    TokenLoss::Revoked => {
                        identity::revoke_api_token(&mut tx, token, writer, now)
                            .await
                            .unwrap();
                    }
                    TokenLoss::Expired => {
                        clock.advance(Duration::seconds(2));
                    }
                    TokenLoss::NarrowScopes => {
                        sqlx::query(
                            "UPDATE api_tokens SET scopes = ARRAY['project.view'] WHERE id = $1",
                        )
                        .bind(token)
                        .execute(&mut *tx)
                        .await
                        .unwrap();
                    }
                    TokenLoss::OutsideSubtree => {
                        sqlx::query("UPDATE api_tokens SET node_id = $2 WHERE id = $1")
                            .bind(token)
                            .bind(other.base.id)
                            .execute(&mut *tx)
                            .await
                            .unwrap();
                    }
                }
                access::bump_epoch(&mut tx).await.unwrap();
                tx.commit().await.unwrap();
            },
        )
        .await;
        let expected = if matches!(loss, TokenLoss::Revoked | TokenLoss::Expired) {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::FORBIDDEN
        };
        assert_eq!(response.status(), expected, "{loss:?}");
        scenario.unchanged().await;
    }
}

// Enough time for the shared build machine to schedule both database clients;
// the assertion waits for PostgreSQL's actual lock state, never a guessed delay.
const LOCK_WAIT_BOUND: WaitDuration = WaitDuration::from_secs(30);

async fn waiting_for_a_lock(pool: &sqlx::PgPool, pid: i32) {
    tokio::time::timeout(LOCK_WAIT_BOUND, async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid = $1 AND NOT granted)",
            )
            .bind(pid)
            .fetch_one(pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the competing transaction reached its database lock");
}

async fn context(scenario: &Scenario) -> Ctx {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let project = scenario.project.clone();
    let mut registry = Registry::new();
    // The handler only hands its context out; the test then checks the guard with it.
    registry.allow_unguarded();
    registry.on(NodeUpdate, move |ctx, (), _| {
        let sender = sender.clone();
        let project = project.clone();
        async move {
            sender.send(ctx).await.unwrap();
            Ok::<_, ApiError>(Node::Project(project))
        }
    });
    let response = registry
        .into_router()
        .with_state(scenario.world.app.app.clone())
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/nodes/{}", scenario.project.base.id))
                .header("authorization", format!("Bearer {}", scenario.writer.token))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&scenario.rename()).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    receiver.recv().await.unwrap()
}

#[tokio::test]
async fn revocation_cannot_commit_between_the_guard_and_its_write() {
    for revoke_session in [false, true] {
        let scenario = Scenario::new().await;
        let ctx = context(&scenario).await;
        let (mut tx, guard) = transaction_guard::begin(&ctx).await.unwrap();
        assert_eq!(guard.user().id(), scenario.writer.id());
        let pool = scenario.world.db.pool.clone();
        let entry = scenario.entry;
        let owner = scenario.owner.id();
        let writer = scenario.writer.id();
        let now = guard.now().get();
        let hash = token_hash(&scenario.writer.token);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut revoke = tokio::spawn(async move {
            let mut tx = pool.begin().await.unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            sender.send(pid).unwrap();
            if revoke_session {
                let session = identity::session_by_access_token(&mut tx, &hash, now)
                    .await
                    .unwrap()
                    .unwrap();
                identity::revoke_session(&mut tx, session.session_id, writer, now)
                    .await
                    .unwrap();
            } else {
                access::remove_entry(&mut tx, entry, owner, now)
                    .await
                    .unwrap();
                access::bump_epoch(&mut tx).await.unwrap();
            }
            tx.commit().await.unwrap();
        });
        let pid = receiver.await.unwrap();
        if revoke_session {
            // The session's row is the action's until it ends: the revocation waits for it.
            waiting_for_a_lock(&scenario.world.db.pool, pid).await;
            assert!(
                !revoke.is_finished(),
                "revocation waits for the authorized action"
            );
        } else {
            // A change of authority waits for nobody's action: it commits now, and the
            // action that decided under the authority it ended cannot commit after it.
            tokio::time::timeout(LOCK_WAIT_BOUND, &mut revoke)
                .await
                .expect("a change of authority does not wait for an open action")
                .unwrap();
        }
        hierarchy::presentation::change(
            &mut tx,
            scenario.project.base.id,
            TextChange {
                name: Some("Renamed"),
                description: None,
            },
            None,
            scenario.project.base.revision,
            now,
        )
        .await
        .unwrap()
        .unwrap();
        if revoke_session {
            tx.commit().await.unwrap();
            revoke.await.unwrap();
            let node =
                places::read(&scenario.world, &scenario.owner, scenario.project.base.id).await;
            let Node::Project(project) = node else {
                panic!("a project")
            };
            assert_eq!(project.base.name, "Renamed");
        } else {
            let refused = tx.commit().await.unwrap_err();
            assert!(
                matches!(orch_db::DbError::from(refused), orch_db::DbError::Retry),
                "the write is refused, retryably, at its commit"
            );
            scenario.unchanged().await;
        }
        let response = request_after(
            &scenario.world,
            &scenario.writer,
            "PATCH",
            &format!("/nodes/{}", scenario.project.base.id),
            scenario.rename(),
            async {},
        )
        .await;
        let expected = if revoke_session {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::FORBIDDEN
        };
        assert_eq!(response.status(), expected);
    }
}

#[tokio::test]
async fn a_guard_waiting_for_revocation_reads_the_new_token_scopes() {
    let mut scenario = Scenario::new().await;
    let token = new_id();
    scenario.writer.token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token(
        &mut *scenario.world.conn().await,
        &NewApiToken {
            id: token,
            user_id: scenario.writer.id(),
            name: "integration",
            token_hash: &token_hash(&scenario.writer.token),
            scopes: &["*".into()],
            node_id: None,
            at: scenario.world.now(),
            expires_at: None,
        },
    )
    .await
    .unwrap();
    let ctx = context(&scenario).await;
    let mut revocation = scenario.world.db.pool.begin().await.unwrap();
    access::lock_system_defaults(&mut revocation).await.unwrap();
    // The token's row is the revocation's until it commits: a guard that authenticates with it waits.
    sqlx::query("UPDATE api_tokens SET scopes = ARRAY['project.view'] WHERE id = $1")
        .bind(token)
        .execute(&mut *revocation)
        .await
        .unwrap();
    // The action uses the API's transaction type, as required by the guard.
    let db = orch_db::Db::from_pool(scenario.world.db.pool.clone());
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let action = tokio::spawn(async move {
        let mut tx = db.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        sender.send(pid).unwrap();
        let error = transaction_guard::check(&ctx, &mut tx).await.unwrap_err();
        tx.rollback().await.unwrap();
        error
    });
    waiting_for_a_lock(&scenario.world.db.pool, receiver.await.unwrap()).await;
    access::bump_epoch(&mut revocation).await.unwrap();
    revocation.commit().await.unwrap();
    let error = action.await.unwrap();
    assert_eq!(
        error.code(),
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
    scenario.unchanged().await;
}

#[tokio::test]
async fn a_guard_samples_the_clock_after_waiting_for_authority_changes() {
    let scenario = Scenario::new().await;
    let now = scenario.world.now();
    sqlx::query("UPDATE permission_entries SET expires_at = $2 WHERE id = $1")
        .bind(scenario.entry)
        .bind(now + Duration::seconds(1))
        .execute(&scenario.world.db.pool)
        .await
        .unwrap();
    let ctx = context(&scenario).await;
    // A change of the writer's standing holds their user row, as a suspension's writer does.
    let mut change = scenario.world.db.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_person(&mut change, scenario.writer.id())
        .await
        .unwrap();
    // The action uses the API's transaction type, as required by the guard.
    let db = orch_db::Db::from_pool(scenario.world.db.pool.clone());
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let action = tokio::spawn(async move {
        let mut tx = db.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        sender.send(pid).unwrap();
        let error = transaction_guard::check(&ctx, &mut tx).await.unwrap_err();
        tx.rollback().await.unwrap();
        error
    });
    waiting_for_a_lock(&scenario.world.db.pool, receiver.await.unwrap()).await;
    scenario.world.advance(Duration::seconds(2));
    change.commit().await.unwrap();
    assert_eq!(
        action.await.unwrap().code(),
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
    scenario.unchanged().await;
}

fn cost_router(scenario: &Scenario, guarded: bool) -> axum::Router {
    let project = scenario.project.clone();
    let mut registry = Registry::new();
    if !guarded {
        // The plain variant is the measured write without its guard.
        registry.allow_unguarded();
    }
    registry.on(NodeUpdate, move |ctx, (), body: NodeUpdateBody| {
        let mut project = project.clone();
        async move {
            let (mut tx, principal) = if guarded {
                let (tx, guard) = transaction_guard::begin(&ctx).await?;
                (tx, guard.principal().clone())
            } else {
                let mut tx = ctx.app.db.begin().await?;
                let principal = orch_authz::Principal::load(
                    &mut tx,
                    authorization::caller_of(ctx.user()?)?,
                    Time::new(ctx.app.clock.now()),
                )
                .await
                .map_err(ApiError::internal)?;
                let principal = orch_api_core::sign_in::second_step::hold(
                    &mut tx,
                    ctx.user()?.as_ref(),
                    principal,
                )
                .await?;
                authorization::require(
                    &mut tx,
                    &principal,
                    ctx.id("id")?,
                    authorization::MANAGE,
                    orch_authz::Act::Use,
                    orch_authz::Policy::default(),
                )
                .await?;
                (tx, principal)
            };
            let revision = hierarchy::presentation::change(
                &mut tx,
                ctx.id("id")?,
                TextChange {
                    name: body.name.as_deref(),
                    description: None,
                },
                None,
                body.expected_revision,
                principal.now().get(),
            )
            .await?
            .ok_or_else(|| ApiError::internal("benchmark revision lost"))?;
            project.base.revision = revision;
            project.base.name = body.name.unwrap();
            tx.commit().await.map_err(orch_db::DbError::from)?;
            Ok::<_, ApiError>(Node::Project(project))
        }
    });
    registry
        .into_router()
        .with_state(scenario.world.app.app.clone())
}

#[tokio::test]
async fn guarding_a_write_adds_one_statement_to_the_same_authority_check() {
    // The requested sample size; alternating the variants shares scheduler/load noise.
    const SAMPLES: usize = 100;
    let scenario = Scenario::new().await;
    let plain = cost_router(&scenario, false);
    let guarded = cost_router(&scenario, true);
    let mut revision = scenario.project.base.revision;
    let mut samples = [Vec::new(), Vec::new()];
    let mut counts = [Vec::new(), Vec::new()];
    for _ in 0..SAMPLES {
        for (index, router) in [&plain, &guarded].into_iter().enumerate() {
            let request = Request::builder()
                .method("PATCH")
                .uri(format!("/nodes/{}", scenario.project.base.id))
                .header("authorization", format!("Bearer {}", scenario.writer.token))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({"name": "Measured", "expected_revision": revision}))
                        .unwrap(),
                ))
                .unwrap();
            let statements = support::Statements::now();
            let start = Instant::now();
            let response = router.clone().oneshot(request).await.unwrap();
            samples[index].push(start.elapsed().as_nanos());
            counts[index].push(support::Statements::now() - statements);
            assert_eq!(response.status(), StatusCode::OK);
            revision += 1;
        }
    }
    for values in &mut samples {
        values.sort_unstable();
    }
    for values in &mut counts {
        values.sort_unstable();
    }
    let median = |values: &[u128]| (values[SAMPLES / 2 - 1] + values[SAMPLES / 2]) / 2;
    println!(
        "guard_cost samples={SAMPLES} plain_us={} guarded_us={} plain_statements={} guarded_statements={}",
        median(&samples[0]) / 1_000,
        median(&samples[1]) / 1_000,
        counts[0][SAMPLES / 2],
        counts[1][SAMPLES / 2]
    );
    assert_eq!(counts[1][SAMPLES / 2], counts[0][SAMPLES / 2] + 1);
}

// Sixteen is the requested installation burst.
const CONCURRENT_WRITERS: usize = 16;
// These proofs hold actions open until another action has finished its write.
// A loaded machine may take longer; this bound only prevents a deadlocked test.
const OVERLAP_PATIENCE: WaitDuration = WaitDuration::from_secs(300);

async fn personal_context(world: &World, who: &Who) -> Ctx {
    let node = places::read(world, who, who.person.personal_org).await;
    let Node::Org(org) = &node else {
        panic!("a personal org")
    };
    let body = serde_json::to_vec(&json!({"expected_revision": org.base.revision})).unwrap();
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let mut registry = Registry::new();
    // The handler only hands its context out; the test then checks the guard with it.
    registry.allow_unguarded();
    registry.on(NodeUpdate, move |ctx, (), _: NodeUpdateBody| {
        let node = node.clone();
        let sender = sender.clone();
        async move {
            sender.send(ctx).await.unwrap();
            Ok::<_, ApiError>(node)
        }
    });
    let response = registry
        .into_router()
        .with_state(world.app.app.clone())
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/nodes/{}", who.person.personal_org))
                .header("authorization", format!("Bearer {}", who.token))
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    receiver.recv().await.unwrap()
}

async fn concurrent_contexts(world: &World) -> Vec<Ctx> {
    let mut contexts = Vec::new();
    for i in 0..CONCURRENT_WRITERS {
        let who = person(world, &format!("writer{i}")).await;
        contexts.push(personal_context(world, &who).await);
    }
    contexts
}

async fn concurrent_world() -> World {
    let mut world = World::new().await;
    // The standard fixture keeps four connections to reduce load across the
    // suite. These tests need sixteen actions and an authority writer in flight.
    let db = orch_db::Db::connect(&world.db.url, (CONCURRENT_WRITERS + 4).try_into().unwrap())
        .await
        .unwrap();
    // A running installation has a warm pool. Establish all sixteen backend
    // connections before timing lock contention, so simultaneous SCRAM/startup
    // work on this shared VM does not masquerade as an epoch-lock queue.
    let mut connections = Vec::with_capacity(CONCURRENT_WRITERS);
    for _ in 0..CONCURRENT_WRITERS {
        connections.push(db.acquire().await.unwrap());
    }
    drop(connections);
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    world.app = orch_api_core::testing::TestApp::new(db, world.app.clock.clone(), registry);
    world
}

async fn guarded_change(ctx: &Ctx, after_write: impl Future<Output = ()>) {
    let (mut tx, guard) = transaction_guard::begin(ctx).await.unwrap();
    // A real non-authority write used by messages and notifications. Its user
    // counter must coexist with identity protection; it is not a suspension.
    sqlx::query("UPDATE users SET inbox_seq = inbox_seq + 1 WHERE id = $1")
        .bind(guard.user().id())
        .execute(&mut *tx)
        .await
        .unwrap();
    after_write.await;
    tx.commit().await.unwrap();
}

async fn burst(contexts: &[Ctx]) {
    let reached = std::sync::Arc::new(tokio::sync::Barrier::new(contexts.len() + 1));
    let release = std::sync::Arc::new(tokio::sync::Barrier::new(contexts.len() + 1));
    let mut tasks = tokio::task::JoinSet::new();
    for ctx in contexts {
        let ctx = ctx.clone();
        let reached = reached.clone();
        let release = release.clone();
        tasks.spawn(async move {
            guarded_change(&ctx, async {
                reached.wait().await;
                release.wait().await;
            })
            .await;
        });
    }
    tokio::time::timeout(OVERLAP_PATIENCE, reached.wait())
        .await
        .expect("all sixteen writes finish before any action may commit");
    release.wait().await;
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
}

#[tokio::test]
async fn ordinary_changes_by_many_people_or_one_person_do_not_queue_on_identity() {
    let world = concurrent_world().await;
    let contexts = concurrent_contexts(&world).await;
    burst(&contexts).await;
    // Two independent changes by the same person, on separate preference keys,
    // so the action's own business row does not cause an expected queue.
    let reached = std::sync::Arc::new(tokio::sync::Barrier::new(3));
    let release = std::sync::Arc::new(tokio::sync::Barrier::new(3));
    let mut tasks = tokio::task::JoinSet::new();
    for key in ["help", "layouts"] {
        let ctx = contexts[0].clone();
        let reached = reached.clone();
        let release = release.clone();
        tasks.spawn(async move {
            let (mut tx, guard) = transaction_guard::begin(&ctx).await.unwrap();
            sqlx::query("INSERT INTO user_state (user_id, key, value) VALUES ($1, $2, '{}')")
                .bind(guard.user().id())
                .bind(key)
                .execute(&mut *tx)
                .await
                .unwrap();
            reached.wait().await;
            release.wait().await;
            tx.commit().await.unwrap();
        });
    }
    tokio::time::timeout(OVERLAP_PATIENCE, reached.wait())
        .await
        .expect("both of one person's writes finish before either action may commit");
    release.wait().await;
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
    let ctx = &contexts[0];
    let (tx, guard) = transaction_guard::begin(ctx).await.unwrap();
    let user = guard.user().id();
    let db = contexts[1].app.db.clone();
    let reference = tokio::spawn(async move {
        let mut tx = db.begin().await.unwrap();
        // A separate writer's real INSERT refers to the protected person. This
        // exercises PostgreSQL's FK key-share lock, independent of rule checks.
        sqlx::query("INSERT INTO user_state (user_id, key, value) VALUES ($1, 'viewers', '{}')")
            .bind(user)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    });
    tokio::time::timeout(OVERLAP_PATIENCE, reference)
        .await
        .expect("the foreign key write commits while the guarded action is still open")
        .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn guarded_changes_keep_their_cost_when_fifteen_other_people_act() {
    // One hundred single observations and one hundred synchronized bursts give
    // both central cost and tail cost on the same database and warmed pool.
    const SAMPLES: usize = 100;
    let world = concurrent_world().await;
    let contexts = concurrent_contexts(&world).await;
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = registry.into_router().with_state(world.app.app.clone());
    let mut alone = Vec::new();
    let mut together = Vec::new();
    api_burst(&contexts, &router).await;
    for _ in 0..SAMPLES {
        let request = measured_request(&contexts[0]).await;
        let start = Instant::now();
        let response = router.clone().oneshot(request).await.unwrap();
        alone.push(start.elapsed().as_micros());
        assert_eq!(response.status(), StatusCode::OK);
        together.extend(api_burst(&contexts, &router).await);
    }
    alone.sort_unstable();
    together.sort_unstable();
    let median = |v: &[u128]| (v[v.len() / 2 - 1] + v[v.len() / 2]) / 2;
    let p95 = |v: &[u128]| v[(v.len() * 95).div_ceil(100) - 1];
    println!(
        "guard_concurrent_cost single_samples={} burst_samples={} alone_median_us={} alone_p95_us={} concurrent_median_us={} concurrent_p95_us={}",
        alone.len(),
        together.len(),
        median(&alone),
        p95(&alone),
        median(&together),
        p95(&together)
    );
}

async fn measured_request(ctx: &Ctx) -> Request<Body> {
    let id = ctx.id("id").unwrap();
    // The caller's already-read revision is prepared outside the timed request.
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM nodes WHERE id = $1")
        .bind(id)
        .fetch_one(ctx.app.db.pool())
        .await
        .unwrap();
    Request::builder()
        .method("PATCH")
        .uri(format!("/nodes/{id}"))
        .header("authorization", ctx.headers.get("authorization").unwrap())
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({
                "name": format!("Measured {}", new_id()), "expected_revision": revision
            }))
            .unwrap(),
        ))
        .unwrap()
}

async fn api_burst(contexts: &[Ctx], router: &axum::Router) -> Vec<u128> {
    let start_line = std::sync::Arc::new(tokio::sync::Barrier::new(contexts.len() + 1));
    let mut tasks = tokio::task::JoinSet::new();
    for ctx in contexts {
        let request = measured_request(ctx).await;
        let router = router.clone();
        let start_line = start_line.clone();
        tasks.spawn(async move {
            start_line.wait().await;
            let start = Instant::now();
            let response = router.oneshot(request).await.unwrap();
            let elapsed = start.elapsed().as_micros();
            assert_eq!(response.status(), StatusCode::OK);
            elapsed
        });
    }
    start_line.wait().await;
    let mut elapsed = Vec::new();
    while let Some(result) = tasks.join_next().await {
        elapsed.push(result.unwrap());
    }
    elapsed
}

#[tokio::test]
async fn all_sixteen_guards_can_reach_their_write_before_any_commits() {
    let world = concurrent_world().await;
    let contexts = concurrent_contexts(&world).await;
    let (ready, mut arrived) = tokio::sync::mpsc::channel(CONCURRENT_WRITERS);
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let mut tasks = tokio::task::JoinSet::new();
    for ctx in contexts {
        let ready = ready.clone();
        let release = release.clone();
        tasks.spawn(async move {
            let (mut tx, guard) = transaction_guard::begin(&ctx).await.unwrap();
            sqlx::query("UPDATE users SET inbox_seq = inbox_seq + 1 WHERE id = $1")
                .bind(guard.user().id())
                .execute(&mut *tx)
                .await
                .unwrap();
            // Register the wait before announcing readiness, so no wake is lost.
            let released = release.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            ready.send(()).await.unwrap();
            released.await;
            tx.commit().await.unwrap();
        });
    }
    let overlap = tokio::time::timeout(WaitDuration::from_secs(30), async {
        for _ in 0..CONCURRENT_WRITERS {
            arrived.recv().await.unwrap();
        }
    })
    .await;
    release.notify_waiters();
    if overlap.is_err() {
        tasks.abort_all();
    }
    while let Some(result) = tasks.join_next().await {
        if overlap.is_ok() {
            result.unwrap();
        }
    }
    assert!(
        overlap.is_ok(),
        "every guard must reach its write while the others are still open"
    );
}

#[tokio::test]
async fn authenticating_the_same_api_token_does_not_wait_for_an_open_action() {
    let world = World::new().await;
    let mut who = person(&world, "tokenwriter").await;
    who.token = new_token(API_TOKEN_PREFIX);
    let token = new_id();
    identity::create_api_token(
        &mut *world.conn().await,
        &NewApiToken {
            id: token,
            user_id: who.id(),
            name: "parallel",
            token_hash: &token_hash(&who.token),
            scopes: &["*".into()],
            node_id: None,
            at: world.now(),
            expires_at: None,
        },
    )
    .await
    .unwrap();
    let ctx = personal_context(&world, &who).await;
    let (tx, _) = transaction_guard::begin(&ctx).await.unwrap();
    let pool = world.db.pool.clone();
    let hash = token_hash(&who.token);
    let now = world.now();
    let authenticate = tokio::spawn(async move {
        identity::api_token_by_hash(&mut pool.acquire().await.unwrap(), &hash, now)
            .await
            .unwrap()
            .unwrap()
    });
    let row = tokio::time::timeout(OVERLAP_PATIENCE, authenticate)
        .await
        .expect("authentication finishes while the guarded action remains open")
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(row.id, token);
    assert_eq!(row.scopes, ["*"]);
    // Once free, another authentication records last use as before.
    world.advance(Duration::seconds(1));
    let later = world.now();
    identity::api_token_by_hash(&mut *world.conn().await, &token_hash(&who.token), later)
        .await
        .unwrap()
        .unwrap();
    let last_used: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT last_used_at FROM api_tokens WHERE id = $1")
            .bind(token)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(last_used, later);
}

#[derive(Debug, Clone, Copy)]
enum AuthorityLoss {
    Suspended,
    ScopeNarrowed,
    TokenExpired,
    TokenRevoked,
    SessionExpired,
    RoleRemoved,
}

async fn prepare_loss(scenario: &mut Scenario, loss: AuthorityLoss) -> Id {
    if matches!(
        loss,
        AuthorityLoss::ScopeNarrowed | AuthorityLoss::TokenExpired | AuthorityLoss::TokenRevoked
    ) {
        let token = new_id();
        scenario.writer.token = new_token(API_TOKEN_PREFIX);
        identity::create_api_token(
            &mut *scenario.world.conn().await,
            &NewApiToken {
                id: token,
                user_id: scenario.writer.id(),
                name: "guarded",
                token_hash: &token_hash(&scenario.writer.token),
                scopes: &["*".into()],
                node_id: None,
                at: scenario.world.now(),
                expires_at: None,
            },
        )
        .await
        .unwrap();
        token
    } else if matches!(loss, AuthorityLoss::RoleRemoved) {
        access::remove_entry(
            &mut *scenario.world.conn().await,
            scenario.entry,
            scenario.owner.id(),
            scenario.world.now(),
        )
        .await
        .unwrap();
        testing::give_role(
            &mut *scenario.world.conn().await,
            scenario.project.base.id,
            "user",
            scenario.writer.id(),
            "admin",
            scenario.owner.id(),
            None,
        )
        .await
        .unwrap()
    } else {
        scenario.writer.id()
    }
}

async fn lose_authority(
    conn: &mut orch_db::Conn,
    writer: Id,
    now: chrono::DateTime<chrono::Utc>,
    loss: AuthorityLoss,
    subject: Id,
) {
    match loss {
        AuthorityLoss::Suspended => {
            identity::set_suspended(conn, subject, true, now)
                .await
                .unwrap();
        }
        AuthorityLoss::ScopeNarrowed => {
            sqlx::query("UPDATE api_tokens SET scopes = ARRAY['project.view'] WHERE id = $1")
                .bind(subject)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        AuthorityLoss::TokenExpired => {
            sqlx::query("UPDATE api_tokens SET expires_at = $2 WHERE id = $1")
                .bind(subject)
                .bind(now)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        AuthorityLoss::TokenRevoked => {
            identity::revoke_api_token(conn, subject, writer, now)
                .await
                .unwrap();
        }
        AuthorityLoss::SessionExpired => {
            sqlx::query("UPDATE sessions SET expires_at = $2 WHERE user_id = $1")
                .bind(subject)
                .bind(now)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        AuthorityLoss::RoleRemoved => {
            sqlx::query("UPDATE node_roles SET removed_at = $2 WHERE id = $1")
                .bind(subject)
                .bind(now)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
    }
    access::bump_epoch(conn).await.unwrap();
}

#[tokio::test]
async fn authority_loss_either_waits_for_the_action_or_is_seen_after_its_commit() {
    for loss in [
        AuthorityLoss::Suspended,
        AuthorityLoss::ScopeNarrowed,
        AuthorityLoss::TokenExpired,
        AuthorityLoss::TokenRevoked,
        AuthorityLoss::SessionExpired,
        AuthorityLoss::RoleRemoved,
    ] {
        // A loss of the person's own standing or credential holds a row the action locks (their
        // user, token or session): one waits for the other. A role is a fact of the place, read
        // under its root org's epoch: nobody waits, and an action that read it before the loss
        // committed cannot commit after it.
        let row_locked = !matches!(loss, AuthorityLoss::RoleRemoved);
        for change_first in [false, true] {
            let mut scenario = Scenario::new().await;
            let subject = prepare_loss(&mut scenario, loss).await;
            let ctx = context(&scenario).await;
            let writer = scenario.writer.id();
            let now = scenario.world.now();
            let error = if change_first && row_locked {
                let mut change = scenario.world.db.pool.begin().await.unwrap();
                orch_db::queries::transaction_guard::lock_authority(&mut change)
                    .await
                    .unwrap();
                lose_authority(&mut change, writer, now, loss, subject).await;
                let (sender, receiver) = tokio::sync::oneshot::channel();
                let checking = ctx.clone();
                let action = tokio::spawn(async move {
                    let mut tx = checking.app.db.begin().await.unwrap();
                    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                        .fetch_one(&mut *tx)
                        .await
                        .unwrap();
                    sender.send(pid).unwrap();
                    let error = transaction_guard::check(&checking, &mut tx)
                        .await
                        .unwrap_err();
                    tx.rollback().await.unwrap();
                    error
                });
                waiting_for_a_lock(&scenario.world.db.pool, receiver.await.unwrap()).await;
                change.commit().await.unwrap();
                action.await.unwrap()
            } else if row_locked {
                let (mut action, _) = transaction_guard::begin(&ctx).await.unwrap();
                let pool = scenario.world.db.pool.clone();
                let (sender, receiver) = tokio::sync::oneshot::channel();
                let change = tokio::spawn(async move {
                    let mut conn = pool.acquire().await.unwrap();
                    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                        .fetch_one(&mut *conn)
                        .await
                        .unwrap();
                    let mut tx = sqlx::Connection::begin(&mut *conn).await.unwrap();
                    sender.send(pid).unwrap();
                    orch_db::queries::transaction_guard::lock_authority(&mut tx)
                        .await
                        .unwrap();
                    lose_authority(&mut tx, writer, now, loss, subject).await;
                    tx.commit().await.unwrap();
                });
                waiting_for_a_lock(&scenario.world.db.pool, receiver.await.unwrap()).await;
                let revision: i64 = sqlx::query_scalar("SELECT revision FROM nodes WHERE id = $1")
                    .bind(scenario.project.base.id)
                    .fetch_one(&mut *action)
                    .await
                    .unwrap();
                hierarchy::presentation::change(
                    &mut action,
                    scenario.project.base.id,
                    TextChange {
                        name: Some("Authorized before loss"),
                        description: None,
                    },
                    None,
                    revision,
                    now,
                )
                .await
                .unwrap()
                .unwrap();
                action.commit().await.unwrap();
                change.await.unwrap();
                transaction_guard::begin(&ctx).await.unwrap_err()
            } else {
                // The action decides while the role still counts, whichever transaction began first.
                let mut change = if change_first {
                    let mut change = scenario.world.db.pool.begin().await.unwrap();
                    orch_db::queries::transaction_guard::lock_authority(&mut change)
                        .await
                        .unwrap();
                    lose_authority(&mut change, writer, now, loss, subject).await;
                    Some(change)
                } else {
                    None
                };
                let (mut action, _) =
                    tokio::time::timeout(LOCK_WAIT_BOUND, transaction_guard::begin(&ctx))
                        .await
                        .expect("an action does not wait for a change of authority")
                        .unwrap();
                match change.take() {
                    Some(change) => change.commit().await.unwrap(),
                    None => {
                        let mut change = scenario.world.db.pool.begin().await.unwrap();
                        lose_authority(&mut change, writer, now, loss, subject).await;
                        tokio::time::timeout(LOCK_WAIT_BOUND, change.commit())
                            .await
                            .expect("a change of authority does not wait for an open action")
                            .unwrap();
                    }
                }
                let revision: i64 = sqlx::query_scalar("SELECT revision FROM nodes WHERE id = $1")
                    .bind(scenario.project.base.id)
                    .fetch_one(&mut *action)
                    .await
                    .unwrap();
                hierarchy::presentation::change(
                    &mut action,
                    scenario.project.base.id,
                    TextChange {
                        name: Some("Authorized before loss"),
                        description: None,
                    },
                    None,
                    revision,
                    now,
                )
                .await
                .unwrap()
                .unwrap();
                let refused = action.commit().await.unwrap_err();
                assert!(
                    matches!(orch_db::DbError::from(refused), orch_db::DbError::Retry),
                    "{loss:?}, change_first={change_first}: the write that read the role is refused at its commit"
                );
                scenario.unchanged().await;
                transaction_guard::begin(&ctx).await.unwrap_err()
            };
            let expected = match loss {
                AuthorityLoss::ScopeNarrowed | AuthorityLoss::RoleRemoved => {
                    orch_api_contract::common::ErrorCode::AuthzDenied
                }
                AuthorityLoss::Suspended => orch_api_contract::common::ErrorCode::AuthSuspended,
                _ => orch_api_contract::common::ErrorCode::AuthInvalidToken,
            };
            assert_eq!(
                error.code(),
                expected,
                "{loss:?}, change_first={change_first}"
            );
        }
    }
}

async fn admin_change(
    router: axum::Router,
    token: String,
    target: Id,
    admin: bool,
    start_line: std::sync::Arc<tokio::sync::Barrier>,
) {
    let request = Request::builder()
        .method("PATCH")
        .uri(format!("/admin/users/{target}"))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({
                "is_system_admin": admin, "acknowledge_admin_risk": true
            }))
            .unwrap(),
        ))
        .unwrap();
    start_line.wait().await;
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn authority_changes_and_ordinary_actions_finish_without_deadlocks_in_a_hundred_rounds() {
    // The requested repetition count exercises both exclusive/exclusive and
    // exclusive/shared schedules, rather than accepting a lucky single order.
    const ROUNDS: usize = 100;
    let world = concurrent_world().await;
    let contexts = concurrent_contexts(&world).await;
    let admin = places::admin(&world, "administrator").await;
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = registry.into_router().with_state(world.app.app.clone());
    let start = Instant::now();
    for round in 0..ROUNDS {
        let start_line = std::sync::Arc::new(tokio::sync::Barrier::new(3));
        let mut tasks = tokio::task::JoinSet::new();
        for ctx in &contexts[..2] {
            tasks.spawn(admin_change(
                router.clone(),
                admin.token.clone(),
                ctx.user().unwrap().id(),
                round % 2 == 0,
                start_line.clone(),
            ));
        }
        start_line.wait().await;
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }

        let start_line = std::sync::Arc::new(tokio::sync::Barrier::new(CONCURRENT_WRITERS + 2));
        let mut tasks = tokio::task::JoinSet::new();
        for ctx in &contexts {
            let ctx = ctx.clone();
            let start_line = start_line.clone();
            tasks.spawn(async move {
                start_line.wait().await;
                // A brief real transaction overlap without adding twenty
                // seconds of intentional delay to the repeated proof.
                guarded_change(&ctx, async {
                    tokio::time::sleep(WaitDuration::from_millis(10)).await;
                })
                .await;
            });
        }
        tasks.spawn(admin_change(
            router.clone(),
            admin.token.clone(),
            contexts[0].user().unwrap().id(),
            round % 2 != 0,
            start_line.clone(),
        ));
        start_line.wait().await;
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }
    }
    println!(
        "guard_authority_stress rounds={ROUNDS} exclusive_pairs={ROUNDS} mixed_bursts={ROUNDS} wall_ms={} deadlocks=0 retries=0",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn an_unannounced_authority_change_moves_the_systems_epoch_at_its_commit() {
    // An action that turns out to change authority holds no shared lock to upgrade:
    // what it changes moves the system's epoch at its commit, and a write that decided under the
    // older one cannot commit after it.
    let scenario = Scenario::new().await;
    let project = scenario.project.clone();
    let entry = scenario.entry;
    let ctx = context(&scenario).await;
    let (mut other, _) = transaction_guard::begin(&ctx).await.unwrap();
    let before: i64 = sqlx::query_scalar("SELECT epoch FROM authz_epochs WHERE org_id = $1")
        .bind(orch_db::queries::transaction_guard::SYSTEM)
        .fetch_one(&scenario.world.db.pool)
        .await
        .unwrap();
    let mut registry = Registry::new();
    registry.on(NodeUpdate, move |ctx, (), _: NodeUpdateBody| {
        let project = project.clone();
        async move {
            let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
            access::remove_entry(&mut tx, entry, guard.user().id(), guard.now().get()).await?;
            access::bump_epoch(&mut tx)
                .await
                .map_err(ApiError::internal)?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            Ok::<_, ApiError>(Node::Project(project))
        }
    });
    let response = registry
        .into_router()
        .with_state(scenario.world.app.app.clone())
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/nodes/{}", scenario.project.base.id))
                .header("authorization", format!("Bearer {}", scenario.owner.token))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&scenario.rename()).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let removed: bool =
        sqlx::query_scalar("SELECT removed_at IS NOT NULL FROM permission_entries WHERE id = $1")
            .bind(entry)
            .fetch_one(&scenario.world.db.pool)
            .await
            .unwrap();
    assert!(removed, "the change was made");
    let after: i64 = sqlx::query_scalar("SELECT epoch FROM authz_epochs WHERE org_id = $1")
        .bind(orch_db::queries::transaction_guard::SYSTEM)
        .fetch_one(&scenario.world.db.pool)
        .await
        .unwrap();
    assert_eq!(after, before + 1, "the system's epoch moved once");
    sqlx::query("UPDATE users SET inbox_seq = inbox_seq + 1 WHERE id = $1")
        .bind(scenario.writer.id())
        .execute(&mut *other)
        .await
        .unwrap();
    let refused = other.commit().await.unwrap_err();
    assert!(matches!(
        orch_db::DbError::from(refused),
        orch_db::DbError::Retry
    ));
}

#[tokio::test]
async fn a_suspension_waits_for_the_users_key_share_even_without_an_epoch_bump() {
    let scenario = Scenario::new().await;
    let ctx = context(&scenario).await;
    let (tx, _) = transaction_guard::begin(&ctx).await.unwrap();
    let pool = scenario.world.db.pool.clone();
    let writer = scenario.writer.id();
    let now = scenario.world.now();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let suspend = tokio::spawn(async move {
        let mut change = pool.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *change)
            .await
            .unwrap();
        sender.send(pid).unwrap();
        identity::set_suspended(&mut change, writer, true, now)
            .await
            .unwrap();
        change.commit().await.unwrap();
    });
    waiting_for_a_lock(&scenario.world.db.pool, receiver.await.unwrap()).await;
    assert!(!suspend.is_finished());
    tx.commit().await.unwrap();
    suspend.await.unwrap();
    assert_eq!(
        transaction_guard::begin(&ctx).await.unwrap_err().code(),
        orch_api_contract::common::ErrorCode::AuthSuspended
    );
}

#[tokio::test]
async fn concurrent_refreshes_and_drafts_pass_another_persons_held_refresh_and_action() {
    use orch_db::queries::transaction_guard::{self as locks, Scope};
    let world = concurrent_world().await;
    let contexts = concurrent_contexts(&world).await;
    let mut cookies = Vec::new();
    for ctx in &contexts {
        let cookie = new_token(orch_api_core::middleware::authentication::SESSION_PREFIX);
        let mut conn = world.conn().await;
        identity::create_session(
            &mut conn,
            &identity::NewSession {
                id: new_id(),
                user_id: ctx.user().unwrap().id(),
                refresh_hash: &token_hash(&cookie),
                user_agent: "concurrency proof",
                ip: None,
                at: world.now(),
                expires_at: world.now() + Duration::days(1),
            },
        )
        .await
        .unwrap();
        cookies.push(cookie);
    }
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = registry.into_router().with_state(world.app.app.clone());
    let held_person = contexts[0].user().unwrap().id();
    let personal = locks::request_scope(
        Scope::Personal {
            user: Some(held_person),
        },
        world.app.app.db.begin(),
    )
    .await
    .unwrap();
    let (action, _) = transaction_guard::begin(contexts.last().unwrap())
        .await
        .unwrap();
    let pending_router = router.clone();
    let pending_cookie = cookies[0].clone();
    let own_refresh = tokio::spawn(async move {
        pending_router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/refresh")
                    .header("cookie", format!("orch_session={pending_cookie}"))
                    .header(orch_api_contract::common::CSRF_HEADER, "1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    });
    // Prove the actual refresh reached its person lock, rather than merely
    // leaving an unpolled future while the other requests run.
    tokio::time::timeout(LOCK_WAIT_BOUND, async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname = current_database()
                   AND query = 'SELECT * FROM users WHERE id = $1 FOR UPDATE' AND wait_event_type = 'Lock')",
            ).fetch_one(&world.db.pool).await.unwrap();
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the refresh waits for its own person");
    let mut tasks = tokio::task::JoinSet::new();
    for index in 1..contexts.len() - 1 {
        let ctx = &contexts[index];
        let request = if index % 2 == 0 {
            Request::builder()
                .method("POST")
                .uri("/auth/refresh")
                .header("cookie", format!("orch_session={}", cookies[index]))
                .header(orch_api_contract::common::CSRF_HEADER, "1")
                .body(Body::empty())
                .unwrap()
        } else {
            let node = ctx.id("id").unwrap();
            let mut conn = world.conn().await;
            let chat = orch_db::queries::chats::chat_of_node(&mut conn, node)
                .await
                .unwrap()
                .unwrap()
                .id;
            Request::builder()
                .method("PUT")
                .uri(format!("/chats/{chat}/draft"))
                .header("authorization", ctx.headers.get("authorization").unwrap())
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&orch_api_contract::chats::ChatDraftSetBody {
                        parts: vec![orch_api_contract::chats::ChatPart::Text {
                            text: "unfinished".to_owned(),
                        }],
                        reply_to: None,
                    })
                    .unwrap(),
                ))
                .unwrap()
        };
        let router = router.clone();
        tasks.spawn(async move {
            let response = router.oneshot(request).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "independent refresh/draft {index}"
            );
        });
    }
    tokio::time::timeout(OVERLAP_PATIENCE, async {
        while let Some(done) = tasks.join_next().await { done.unwrap(); }
    }).await.expect("all other people's refreshes and drafts finish while both original transactions remain open");
    assert!(
        !own_refresh.is_finished(),
        "the same person's refresh still waits"
    );
    action.commit().await.unwrap();
    personal.commit().await.unwrap();
    assert_eq!(own_refresh.await.unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn creating_an_object_can_invalidate_caches_without_blocking_an_unrelated_action() {
    use orch_db::queries::transaction_guard::{self as locks, Scope};
    let scenario = Scenario::new().await;
    let ctx = context(&scenario).await;
    let before = {
        let mut conn = scenario.world.conn().await;
        access::epoch(&mut conn).await.unwrap()
    };
    let mut birth = locks::request_scope(Scope::Creation, ctx.app.db.begin())
        .await
        .unwrap();
    assert!(access::bump_epoch(&mut birth).await.unwrap() > before);
    let (action, _) = tokio::time::timeout(LOCK_WAIT_BOUND, transaction_guard::begin(&ctx))
        .await
        .expect("the cache-counter writer is not the authorization barrier")
        .unwrap();
    action.commit().await.unwrap();
    birth.commit().await.unwrap();
    let mut personal = locks::request_scope(
        Scope::Personal {
            user: Some(ctx.user().unwrap().id()),
        },
        ctx.app.db.begin(),
    )
    .await
    .unwrap();
    let epoch = access::epoch(&mut personal).await.unwrap();
    assert_eq!(
        access::bump_epoch(&mut personal).await.unwrap(),
        epoch,
        "personal credentials are checked from locked rows, not cached permission facts"
    );
    personal.commit().await.unwrap();
}

#[tokio::test]
async fn an_unrelated_permission_change_does_not_wait_for_a_personal_credential_transaction() {
    use orch_db::queries::transaction_guard::{self as locks, Scope};
    let scenario = Scenario::new().await;
    let ctx = context(&scenario).await;
    let other = person(&scenario.world, "unrelated").await;
    let db = &scenario.world.app.app.db;
    let personal = locks::request_scope(
        Scope::Personal {
            user: Some(other.id()),
        },
        db.begin(),
    )
    .await
    .unwrap();
    let mut permission = tokio::time::timeout(
        LOCK_WAIT_BOUND,
        locks::request_scope(Scope::Authority, db.begin()),
    )
    .await
    .expect("a personal credential writer does not hold the global barrier")
    .unwrap();
    access::remove_entry(
        &mut permission,
        scenario.entry,
        scenario.owner.id(),
        scenario.world.now(),
    )
    .await
    .unwrap();
    access::bump_epoch(&mut permission).await.unwrap();
    permission.commit().await.unwrap();
    personal.commit().await.unwrap();
    assert_eq!(
        transaction_guard::begin(&ctx).await.unwrap_err().code(),
        orch_api_contract::common::ErrorCode::AuthzDenied
    );
}

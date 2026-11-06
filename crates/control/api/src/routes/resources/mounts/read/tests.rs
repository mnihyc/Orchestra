//! Actual HTTP origins through the generic reader and unchanged native Mount factory.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use super::*;
use crate::middleware::authentication::{
    ACCESS_TOKEN_PREFIX, API_TOKEN_PREFIX, new_token, token_hash,
};
use crate::support::World;
use crate::testing::TestApp;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::{ErrorCode, MAX_JSON_BYTES, Problem};
use orch_api_contract::mounts::{MountGet, MountList};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::{identity, resources::access_fixtures};
use orch_types::{ManualClock, new_id};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

const PRIVATE: &str = "held-native-mount-content";

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    RenderSuccess,
    RenderError,
    AfterGet,
    NativeFrame,
}

struct Cut {
    clock: ManualClock,
    stage: Stage,
    reached: Arc<AtomicUsize>,
}

tokio::task_local! { static CUT: Cut; }

/// The test changes time only after the real native renderer, or inside the existing
/// once-after-commit hook. Its factory, query filter, gates and payload remain native.
struct NativeCut;

impl Family for NativeCut {
    type Item = api::Mount;
    type ListQuery = api::MountListQuery;
    const SPEC: &'static resource_reads::Family = <Mount as Family>::SPEC;
    const CARDINALITY: Cardinality = <Mount as Family>::CARDINALITY;

    fn require_get<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        id: Id,
    ) -> BoxFuture<'a, ApiResult<()>> {
        <Mount as Family>::require_get(conn, principal, id)
    }

    fn prepare_get<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        id: Id,
    ) -> BoxFuture<'a, ApiResult<projection::Prepared>> {
        Box::pin(async move {
            CUT.with(|cut| {
                if cut.stage == Stage::NativeFrame {
                    assert!(
                        !ctx.user().unwrap().as_admin,
                        "the opening Session mode is not the native rendering context"
                    );
                    assert!(!principal.caller().as_admin());
                    assert_eq!(ctx.now, principal.now().get());
                    assert!(authorization::same_credential_origin(
                        ctx.user().unwrap(),
                        principal.caller()
                    ));
                    cut.reached.fetch_add(1, Ordering::SeqCst);
                }
            });
            <Mount as Family>::prepare_get(ctx, conn, principal, id).await
        })
    }

    fn filter<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        rows: Vec<Seen>,
        query: &'a Self::ListQuery,
    ) -> BoxFuture<'a, ApiResult<Vec<Seen>>> {
        <Mount as Family>::filter(conn, principal, rows, query)
    }

    fn into_items(rendered: Rendered) -> ApiResult<Vec<Self::Item>> {
        let rows = <Mount as Family>::into_items(rendered)?;
        let fail = CUT.with(|cut| {
            if matches!(cut.stage, Stage::RenderSuccess | Stage::RenderError) {
                assert_eq!(
                    cut.reached.fetch_add(1, Ordering::SeqCst),
                    0,
                    "one actual native render reaches the cut"
                );
                cut.clock.advance(chrono::Duration::seconds(2));
            }
            cut.stage == Stage::RenderError
        });
        if fail {
            Err(ApiError::new(ErrorCode::AuthzDenied, PRIVATE).with("native", PRIVATE))
        } else {
            Ok(rows)
        }
    }

    fn after_get<'a>(ctx: &'a Ctx, item: &'a Self::Item) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            <Mount as Family>::after_get(ctx, item).await?;
            CUT.with(|cut| {
                if cut.stage == Stage::AfterGet {
                    assert_eq!(
                        cut.reached.fetch_add(1, Ordering::SeqCst),
                        0,
                        "native auxiliary work is never replayed"
                    );
                    cut.clock.advance(chrono::Duration::seconds(2));
                    Err(ApiError::new(ErrorCode::AuthzDenied, PRIVATE).with("native", PRIVATE))
                } else {
                    Ok(())
                }
            })
        })
    }
}

#[derive(Clone, Copy)]
enum Origin {
    Session,
    Legacy,
    Scoped,
}

async fn credential(world: &World, user: Id, mount: Id, origin: Origin) -> String {
    let secret = new_token(if matches!(origin, Origin::Session) {
        ACCESS_TOKEN_PREFIX
    } else {
        API_TOKEN_PREFIX
    });
    let hash = token_hash(&secret);
    let end = world.now() + chrono::Duration::seconds(1);
    if matches!(origin, Origin::Session) {
        identity::testing::insert_session(&mut *world.conn().await, new_id(), user, &hash, end)
            .await
            .unwrap();
    } else {
        let scope = if matches!(origin, Origin::Scoped) {
            let scope: KeyScope = serde_json::from_value(json!({"rules":[{
                "target":{"kind":"mount","id":mount,"reach":"self"},
                "allow":[{"operation":"MountGet","actions":["read"]},{"operation":"MountList","actions":["read"]}]
            }]})).unwrap();
            Some(serde_json::to_value(scope).unwrap())
        } else {
            None
        };
        let legacy = if scope.is_some() {
            vec![]
        } else {
            vec!["mount.view".to_owned()]
        };
        identity::create_api_token_scoped(
            &mut *world.conn().await,
            &identity::NewApiToken {
                id: new_id(),
                user_id: user,
                name: "Native read cut",
                token_hash: &hash,
                scopes: &legacy,
                node_id: None,
                at: world.now(),
                expires_at: Some(end),
            },
            scope.as_ref(),
        )
        .await
        .unwrap();
    }
    secret
}

async fn mount_fixture(world: &World, owner: &crate::places::Who) -> Id {
    let mut conn = world.conn().await;
    let id = access_fixtures::insert_resource(
        &mut conn,
        "mount",
        Some(owner.id()),
        Some(owner.person.personal_org),
        PRIVATE,
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Mount.into(),
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

fn reader(world: &World) -> TestApp {
    let mut registry = Registry::new();
    read::register::<NativeCut, _, _>(&mut registry, MountGet, MountList);
    TestApp::over(world.app.app.clone(), world.app.clock.clone(), registry)
}

#[tokio::test]
async fn actual_session_legacy_and_scoped_get_list_hold_native_bodies_and_errors_until_postcommit_deadline()
 {
    let world = World::new().await;
    let owner = crate::places::person(&world, "read-cut-owner").await;
    let mount = mount_fixture(&world, &owner).await;
    let app = reader(&world);
    for origin in [Origin::Session, Origin::Legacy, Origin::Scoped] {
        for stage in [Stage::RenderSuccess, Stage::RenderError] {
            for list in [false, true] {
                let token = credential(&world, owner.id(), mount, origin).await;
                let reached = Arc::new(AtomicUsize::new(0));
                let reply = CUT
                    .scope(
                        Cut {
                            clock: world.app.clock.clone(),
                            stage,
                            reached: reached.clone(),
                        },
                        async {
                            if list {
                                app.call(MountList, &[]).token(&token).send_bytes().await
                            } else {
                                app.call(MountGet, &[&mount])
                                    .token(&token)
                                    .send_bytes()
                                    .await
                            }
                        },
                    )
                    .await;
                assert_eq!(
                    reached.load(Ordering::SeqCst),
                    1,
                    "the actual native renderer reached the cut"
                );
                assert_eq!(reply.status, http::StatusCode::UNAUTHORIZED);
                let problem: Problem = serde_json::from_slice(&reply.raw).unwrap();
                assert_eq!(problem.code, ErrorCode::AuthInvalidToken);
                assert!(!String::from_utf8_lossy(&reply.raw).contains(PRIVATE));
            }
        }
    }
}

#[tokio::test]
async fn actual_native_get_auxiliary_hook_runs_once_outside_retry_and_keeps_errors_behind_the_final_cut()
 {
    let world = World::new().await;
    let owner = crate::places::person(&world, "read-hook-owner").await;
    let mount = mount_fixture(&world, &owner).await;
    let app = reader(&world);
    for origin in [Origin::Session, Origin::Legacy, Origin::Scoped] {
        let token = credential(&world, owner.id(), mount, origin).await;
        let reached = Arc::new(AtomicUsize::new(0));
        let reply = CUT
            .scope(
                Cut {
                    clock: world.app.clock.clone(),
                    stage: Stage::AfterGet,
                    reached: reached.clone(),
                },
                app.call(MountGet, &[&mount]).token(&token).send_bytes(),
            )
            .await;
        assert_eq!(reached.load(Ordering::SeqCst), 1);
        assert_eq!(reply.status, http::StatusCode::UNAUTHORIZED);
        assert!(!String::from_utf8_lossy(&reply.raw).contains(PRIVATE));
    }
}

#[tokio::test]
async fn native_get_context_uses_actual_session_mode_and_the_same_principal_time_after_opening() {
    use axum::body::{Body, to_bytes};
    use http::Request;
    use tower::ServiceExt;
    // Scheduling headroom only: the witnessed PostgreSQL identity lock decides the cut.
    let watchdog = std::time::Duration::from_secs(30);
    let world = World::new().await;
    let owner = crate::places::admin(&world, "current-read-owner").await;
    let mount = mount_fixture(&world, &owner).await;
    let session: Id =
        sqlx::query_scalar("SELECT session_id FROM access_tokens WHERE token_hash=$1")
            .bind(token_hash(&owner.token))
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    let mut blocker = world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM sessions WHERE id=$1 FOR UPDATE")
        .bind(session)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let mut registry = Registry::new();
    read::register::<NativeCut, _, _>(&mut registry, MountGet, MountList);
    let app = registry
        .into_router_for(orch_api_contract::surface::HttpSurface::Browser)
        .with_state(world.app.app.clone());
    let reached = Arc::new(AtomicUsize::new(0));
    let mut action = tokio::spawn(
        CUT.scope(
            Cut {
                clock: world.app.clock.clone(),
                stage: Stage::NativeFrame,
                reached: reached.clone(),
            },
            app.oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/mounts/{mount}"))
                    .header("authorization", format!("Bearer {}", owner.token))
                    .body(Body::empty())
                    .unwrap(),
            ),
        ),
    );
    let waiting = tokio::time::timeout(watchdog, async {
        loop {
            if action.is_finished() {
                return false;
            }
            let witnessed: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                  WHERE datname=current_database() AND wait_event_type='Lock'
                    AND query LIKE $2 AND $1=ANY(pg_blocking_pids(pid)))",
            )
            .bind(blocker_pid)
            .bind("%FOR KEY SHARE OF u FOR SHARE OF s, a%")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
            if witnessed {
                return true;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    if !matches!(waiting, Ok(true)) {
        blocker.rollback().await.unwrap();
        action.abort();
        let _ = action.await;
        panic!("the real Session identity checkpoint was not witnessed; no mode change applied");
    }
    sqlx::query("UPDATE sessions SET as_admin=false WHERE id=$1")
        .bind(session)
        .execute(&mut *blocker)
        .await
        .unwrap();
    world.advance(chrono::Duration::seconds(1));
    blocker.commit().await.unwrap();
    let response = match tokio::time::timeout(watchdog, &mut action).await {
        Ok(response) => response.unwrap().unwrap(),
        Err(_) => {
            action.abort();
            let _ = action.await;
            panic!("the released actual Session read did not complete");
        }
    };
    assert_eq!(response.status(), http::StatusCode::OK);
    let bytes = to_bytes(response.into_body(), MAX_JSON_BYTES)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains(PRIVATE));
    assert_eq!(reached.load(Ordering::SeqCst), 1);
}

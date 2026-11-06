//! Raw HTTP controls stay compatible with the full-only baseline and typed standing receipts.
use super::*;
use axum::body::{Body, to_bytes};
use bytes::Bytes;
use futures::stream;
use http::{Request, StatusCode};
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::registry::Registry;
use orch_api_core::testing::RawReply;
use orch_authz::standing::{self, Acting};
use orch_db::audit::{Actor, Origin};
use orch_db::queries::identity;
use orch_types::{Json, new_id};
use std::future::Future;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tower::ServiceExt;

const OPERATION: &str = "AdminUserUpdate";

struct Key {
    id: Id,
    secret: String,
}

fn rule(account: Id, operation: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":"account","id":account,"reach":"self"},
        "allow":[{"operation":operation,"actions":actions}]})
}

async fn key(world: &World, actor: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let id = new_id();
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: actor,
            name: "Exact account standing control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    Key { id, secret }
}

async fn change(world: &World, token: &str, account: Id, body: Json) -> RawReply {
    world
        .app
        .call(AdminUserUpdate, &[&account])
        .token(token)
        .raw_body(&body)
        .send_bytes()
        .await
}

fn body(reply: &RawReply) -> Json {
    serde_json::from_slice(&reply.raw).unwrap()
}

fn accepted(reply: &RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    body(reply)
}

async fn state(world: &World, account: Id) -> (String, bool) {
    sqlx::query_as("SELECT status,is_system_admin FROM users WHERE id=$1")
        .bind(account)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

async fn effects(world: &World, account: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action IN ('user.suspended','user.resumed','user.admin_granted','user.admin_revoked')")
        .bind(account).fetch_one(&world.db.pool).await.unwrap()
}

async fn no_dispatch(world: &World) {
    let count: i64 =
        sqlx::query_scalar("SELECT (SELECT count(*) FROM jobs)+(SELECT count(*) FROM commands)")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        count, 0,
        "standing changes and their refusals do not enqueue native work"
    );
}

#[tokio::test]
async fn manage_receipt_and_same_operation_read_preserve_the_existing_standing_effect() {
    let world = World::new().await;
    let c = cast(&world).await;
    let target = c.ada.id();
    let manage = key(
        &world,
        c.root.id(),
        vec![rule(target, OPERATION, &["manage"])],
    )
    .await;
    let receipt = accepted(
        &change(
            &world,
            &manage.secret,
            target,
            json!({"status":"suspended"}),
        )
        .await,
    );
    assert_eq!(receipt, json!({"projection":"receipt","account_id":target}));
    assert_eq!(state(&world, target).await, ("suspended".into(), false));
    assert_eq!(effects(&world, target).await, 1);
    assert!(!receipt.to_string().contains("email"));
    let read = key(
        &world,
        c.root.id(),
        vec![rule(target, OPERATION, &["manage", "read"])],
    )
    .await;
    let full = accepted(&change(&world, &read.secret, target, json!({"status":"active"})).await);
    assert_eq!(full["id"], json!(target));
    assert_eq!(full["status"], "active");
    assert_eq!(full["email"], "ada@example.org");
    assert!(full.get("projection").is_none());
    assert_eq!(effects(&world, target).await, 2);
    no_dispatch(&world).await;
}

#[tokio::test]
async fn another_endpoint_action_or_account_cannot_authorize_the_standing_write() {
    let world = World::new().await;
    let c = cast(&world).await;
    for scope in [
        rule(c.ada.id(), "AdminUserResetLinkCreate", &["manage", "read"]),
        rule(c.ada.id(), OPERATION, &["read"]),
        rule(c.boss.id(), OPERATION, &["manage", "read"]),
    ] {
        let token = key(&world, c.root.id(), vec![scope]).await;
        let reply = change(
            &world,
            &token.secret,
            c.ada.id(),
            json!({"status":"suspended"}),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::FORBIDDEN,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
        assert_eq!(state(&world, c.ada.id()).await, ("active".into(), false));
        assert_eq!(effects(&world, c.ada.id()).await, 0);
    }
    let nonadmin = key(
        &world,
        c.boss.id(),
        vec![rule(c.ada.id(), OPERATION, &["manage", "read"])],
    )
    .await;
    let reply = change(
        &world,
        &nonadmin.secret,
        c.ada.id(),
        json!({"status":"suspended"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    let problem = reply.problem();
    assert_eq!(problem.code, ErrorCode::AuthzDenied);
    assert_eq!(
        problem.extra.get("required"),
        Some(&json!("system.admin")),
        "key selectors cannot add ordinary standing"
    );
    assert_eq!(effects(&world, c.ada.id()).await, 0);
    no_dispatch(&world).await;
}

async fn after_body(
    world: &World,
    token: &str,
    account: Id,
    value: Json,
    alteration: impl Future<Output = ()> + Send + 'static,
) -> (StatusCode, Json, bool) {
    let polled = Arc::new(AtomicBool::new(false));
    let recorded = Arc::clone(&polled);
    let request = Request::builder()
        .method("PATCH")
        .uri(format!("/admin/users/{account}"))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from_stream(stream::once(async move {
            alteration.await;
            recorded.store(true, Ordering::SeqCst);
            Ok::<_, std::io::Error>(Bytes::from(serde_json::to_vec(&value).unwrap()))
        })))
        .unwrap();
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    // Same thirty-second request watchdog as the existing kguard_admin body seam.
    let reply = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        registry
            .into_router_for(
                orch_api_contract::surface::HttpSurface::for_token(token)
                    .expect("known fixture credential"),
            )
            .with_state(world.app.app.clone())
            .oneshot(request),
    )
    .await
    .expect("the guarded body request completes")
    .unwrap();
    let status = reply.status();
    let bytes = to_bytes(reply.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap(),
        polled.load(Ordering::SeqCst),
    )
}

#[tokio::test]
async fn the_key_and_ordinary_admin_standing_are_current_after_the_body() {
    for revoked in [false, true] {
        let world = World::new().await;
        let c = cast(&world).await;
        let writer = places::admin(&world, "writer").await;
        let token = key(
            &world,
            writer.id(),
            vec![rule(c.ada.id(), OPERATION, &["manage", "read"])],
        )
        .await;
        let (pool, at, actor, owner, key_id) = (
            world.db.pool.clone(),
            world.now(),
            writer.id(),
            c.root.id(),
            token.id,
        );
        let (status, problem, polled) = after_body(
            &world,
            &token.secret,
            c.ada.id(),
            json!({"status":"suspended"}),
            async move {
                let mut tx = pool.begin().await.unwrap();
                if revoked {
                    assert!(
                        identity::revoke_api_token(&mut tx, key_id, actor, at)
                            .await
                            .unwrap()
                    );
                } else {
                    standing::revoke_admin(
                        &mut tx,
                        actor,
                        &Acting {
                            by: Actor::User(owner),
                            origin: &Origin::default(),
                            now: at,
                        },
                    )
                    .await
                    .unwrap();
                }
                tx.commit().await.unwrap();
            },
        )
        .await;
        assert!(
            polled,
            "the actual body change must occur before the current guard"
        );
        assert!(matches!(status.as_u16(), 401 | 403), "{problem}");
        assert_eq!(state(&world, c.ada.id()).await, ("active".into(), false));
        assert_eq!(effects(&world, c.ada.id()).await, 0);
        no_dispatch(&world).await;
    }
}

#[tokio::test]
async fn accepted_self_demotion_or_suspension_keeps_its_receipt_and_drops_current_admin_content() {
    for suspended in [false, true] {
        let world = World::new().await;
        let c = cast(&world).await;
        world.make_admin(&c.boss.person).await;
        let token = key(
            &world,
            c.root.id(),
            vec![rule(c.root.id(), OPERATION, &["manage", "read"])],
        )
        .await;
        let value = if suspended {
            json!({"status":"suspended","acknowledge_admin_risk":true})
        } else {
            json!({"is_system_admin":false,"acknowledge_admin_risk":true})
        };
        let reply = change(&world, &token.secret, c.root.id(), value).await;
        assert_eq!(
            accepted(&reply),
            json!({"projection":"receipt","account_id":c.root.id()})
        );
        let changed = state(&world, c.root.id()).await;
        assert_eq!(
            changed,
            if suspended {
                ("suspended".into(), true)
            } else {
                ("active".into(), false)
            }
        );
        assert_eq!(effects(&world, c.root.id()).await, 1);
        let again = change(&world, &token.secret, c.root.id(), json!({})).await;
        assert!(
            matches!(again.status.as_u16(), 401 | 403),
            "the same credential cannot regain current administration"
        );
        no_dispatch(&world).await;
    }
}

#[tokio::test]
async fn modern_standing_keys_keep_risk_acknowledgement_and_last_admin_guards() {
    let world = World::new().await;
    let c = cast(&world).await;
    let token = key(
        &world,
        c.root.id(),
        vec![rule(c.root.id(), OPERATION, &["manage", "read"])],
    )
    .await;
    for (value, code) in [
        (
            json!({"is_system_admin":false}),
            ErrorCode::AdminRiskNotAcknowledged,
        ),
        (
            json!({"is_system_admin":false,"acknowledge_admin_risk":true}),
            ErrorCode::AdminLastAdmin,
        ),
    ] {
        let reply = change(&world, &token.secret, c.root.id(), value).await;
        assert_eq!(reply.problem().code, code);
        assert_eq!(state(&world, c.root.id()).await, ("active".into(), true));
        assert_eq!(effects(&world, c.root.id()).await, 0);
    }
    no_dispatch(&world).await;
}

#[tokio::test]
async fn exact_account_scope_does_not_turn_a_service_identity_into_an_admin_person_target() {
    let world = World::new().await;
    let c = cast(&world).await;
    let service = world
        .app
        .call(
            orch_api_contract::directory::OrgServiceAccountCreate,
            &[&c.acme],
        )
        .token(&c.boss.token)
        .body(&orch_api_contract::directory::OrgServiceAccountCreateBody {
            username: "standing-service".into(),
            display_name: "Standing service".into(),
        })
        .send()
        .await
        .ok()
        .user
        .id;
    for account in [service, new_id()] {
        let token = key(
            &world,
            c.root.id(),
            vec![rule(account, OPERATION, &["manage", "read"])],
        )
        .await;
        let reply = change(
            &world,
            &token.secret,
            account,
            json!({"status":"suspended"}),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::NOT_FOUND,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        assert_eq!(reply.problem().code, ErrorCode::NotFound);
        assert_eq!(effects(&world, account).await, 0);
    }
    assert_eq!(state(&world, service).await, ("active".into(), false));
    no_dispatch(&world).await;
}

//! Exact Account scope restricts the existing keeper-authorized, empty-reply deletion.
use super::*;
use axum::body::{Body, to_bytes};
use http::{Request, StatusCode};
use orch_access_contract::key_scope::{KeyPlaceExclusion, KeyScope};
use orch_access_contract::roles::PlaceRef;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::registry::Registry;
use orch_api_core::testing::RawReply;
use orch_db::queries::{access, identity};
use orch_types::{Json, new_id};
use serde_json::json;
use tower::ServiceExt;

// Reuse the existing bodyless guard tests' scheduling bound; a PostgreSQL blocker,
// rather than elapsed time, decides when the request has reached its current guard.
const REQUEST_BOUND: std::time::Duration = std::time::Duration::from_secs(30);

fn account_rule(account: Id, operation: &str, action: &str) -> Json {
    json!({"target":{"kind":"account","id":account,"reach":"self"},
        "resources":["account"],"allow":[{"operation":operation,"actions":[action]}]})
}

async fn scoped(world: &World, actor: Id, rules: Vec<Json>, excluded: Option<Id>) -> String {
    let mut scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    if let Some(place) = excluded {
        scope.excluded_places.push(KeyPlaceExclusion::Refused {
            place: PlaceRef::Node(place),
        });
    }
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: actor,
            name: "Service deletion boundary",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    secret
}

async fn delete(world: &World, secret: &str, account: Id) -> RawReply {
    world
        .app
        .call(ServiceAccountDelete, &[&account])
        .token(secret)
        .send_bytes()
        .await
}

fn empty_success(reply: RawReply) {
    assert_eq!(
        reply.status,
        StatusCode::NO_CONTENT,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert!(
        reply.raw.is_empty(),
        "a completed deletion needs no Account Read"
    );
}

fn refused(reply: RawReply, code: ErrorCode) {
    let body: Json = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(body["code"], code.as_str(), "{body}");
    assert!(!reply.status.is_success());
}

async fn state(world: &World, account: Id) -> Json {
    sqlx::query_scalar(
        "SELECT jsonb_build_object('status',u.status,
          'tokens',(SELECT count(*) FROM api_tokens WHERE user_id=u.id AND revoked_at IS NULL),
          'roles',(SELECT count(*) FROM node_roles WHERE subject_kind='user' AND subject_id=u.id AND removed_at IS NULL),
          'entries',(SELECT count(*) FROM permission_entries WHERE holder_kind='user' AND holder_id=u.id AND removed_at IS NULL),
          'deletions',(SELECT count(*) FROM audit_log WHERE action='service_account.deleted' AND object_id=u.id))
         FROM users u WHERE u.id=$1",
    )
    .bind(account)
    .fetch_one(&world.db.pool)
    .await
    .unwrap()
}

async fn seeded(world: &World, s: &Setup, username: &str) -> (Id, String) {
    let service = make(world, &s.boss, s.acme, username, "Deletion target")
        .await
        .ok()
        .user
        .id;
    give(world, &s.boss, s.web, service, Role::Developer).await;
    let child = token(world, &s.boss, service, &named("Target credential"))
        .await
        .ok()
        .token
        .unwrap();
    access::testing::add_entry(
        &mut *world.conn().await,
        &access::PageKey::new("node", s.docs, "live"),
        "any",
        "project.view",
        ("user", Some(service), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    let before = state(world, service).await;
    assert_eq!(before["status"], "active");
    for field in ["tokens", "roles", "entries"] {
        assert!(before[field].as_i64().unwrap() > 0, "{field}: {before}");
    }
    (service, child)
}

async fn disabled(world: &World, service: Id, child: &str, node: Id) {
    let after = state(world, service).await;
    assert_eq!(after["status"], "suspended");
    for field in ["tokens", "roles", "entries"] {
        assert_eq!(after[field], 0, "{field}: {after}");
    }
    assert_eq!(after["deletions"], 1);
    assert_eq!(read_as(world, child, node).await.status, 401);
}

#[tokio::test]
async fn exact_account_or_keeper_ancestor_manage_deletes_with_all_existing_effects() {
    let world = World::new().await;
    let s = setup(&world).await;
    for (username, ancestor) in [("exact-scope-target", false), ("keeper-scope-target", true)] {
        let (service, child) = seeded(&world, &s, username).await;
        let rule = if ancestor {
            json!({"target":{"kind":"org","id":s.acme,"reach":"subtree"},
                "resources":["account"],"allow":[{"operation":"ServiceAccountDelete","actions":["manage"]}]})
        } else {
            account_rule(service, "ServiceAccountDelete", "manage")
        };
        let secret = scoped(&world, s.boss.id(), vec![rule], None).await;
        empty_success(delete(&world, &secret, service).await);
        disabled(&world, service, &child, s.web).await;
    }
}

#[tokio::test]
async fn deletion_scope_never_substitutes_another_operation_account_action_or_keeper_right() {
    let world = World::new().await;
    let s = setup(&world).await;
    let (service, _) = seeded(&world, &s, "bounded-delete-target").await;
    let other = make(&world, &s.boss, s.acme, "other-delete-target", "Other")
        .await
        .ok()
        .user
        .id;
    let before = state(&world, service).await;
    for (actor, rule, exclusion) in [
        (
            s.boss.id(),
            account_rule(service, "OrgServiceAccountCreate", "manage"),
            None,
        ),
        (
            s.boss.id(),
            account_rule(service, "ServiceAccountDelete", "read"),
            None,
        ),
        (
            s.boss.id(),
            account_rule(other, "ServiceAccountDelete", "manage"),
            None,
        ),
        (
            s.boss.id(),
            account_rule(service, "ServiceAccountDelete", "manage"),
            Some(s.acme),
        ),
        (
            s.dan.id(),
            account_rule(service, "ServiceAccountDelete", "manage"),
            None,
        ),
    ] {
        let secret = scoped(&world, actor, vec![rule], exclusion).await;
        refused(
            delete(&world, &secret, service).await,
            ErrorCode::AuthzDenied,
        );
        assert_eq!(state(&world, service).await, before);
    }
    let exact_human = scoped(
        &world,
        s.boss.id(),
        vec![account_rule(s.dan.id(), "ServiceAccountDelete", "manage")],
        None,
    )
    .await;
    refused(
        delete(&world, &exact_human, s.dan.id()).await,
        ErrorCode::NotFound,
    );
    let status: String = sqlx::query_scalar("SELECT status FROM users WHERE id=$1")
        .bind(s.dan.id())
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(status, "active");
}

#[tokio::test]
async fn a_bodyless_delete_rechecks_keeper_members_manage_after_its_identity_wait() {
    let world = World::new().await;
    let s = setup(&world).await;
    let (service, _) = seeded(&world, &s, "waited-delete-target").await;
    let probe = make(&world, &s.boss, s.acme, "waited-delete-probe", "Probe")
        .await
        .ok()
        .user
        .id;
    let entry = access::testing::add_entry(
        &mut *world.conn().await,
        &access::PageKey::new("node", s.acme, "live"),
        "any",
        "members.manage",
        ("user", Some(s.dan.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
    let secret = scoped(
        &world,
        s.dan.id(),
        vec![
            account_rule(probe, "ServiceAccountDelete", "manage"),
            account_rule(service, "ServiceAccountDelete", "manage"),
        ],
        None,
    )
    .await;
    // A real positive first makes the BEFORE failure an admission gap, rather than
    // allowing a missing producer's early 403 to impersonate the guarded refusal.
    empty_success(delete(&world, &secret, probe).await);
    let before = state(&world, service).await;
    let mut held = world.db.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_person(&mut held, s.dan.id())
        .await
        .unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = registry
        .into_router_for(orch_api_contract::surface::HttpSurface::Key)
        .with_state(world.app.app.clone());
    let request = Request::builder()
        .method("DELETE")
        .uri(format!("/service-accounts/{service}"))
        .header("authorization", format!("Bearer {secret}"))
        .body(Body::empty())
        .unwrap();
    let mut action = tokio::spawn(async move { router.oneshot(request).await.unwrap() });
    let observed = tokio::time::timeout(REQUEST_BOUND, async {
        loop {
            if action.is_finished() {
                return false;
            }
            let waits: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))",
            ).bind(blocker).fetch_one(&world.db.pool).await.unwrap();
            if waits { return true; }
            tokio::task::yield_now().await;
        }
    }).await;
    if !matches!(observed, Ok(true)) {
        action.abort();
        let _ = action.await;
        held.rollback().await.unwrap();
        panic!("the bodyless request did not reach its identity guard");
    }
    assert!(
        access::remove_entry(&mut held, entry, s.boss.id(), world.now())
            .await
            .unwrap()
    );
    orch_authz::epoch::bump(&mut held).await.unwrap();
    held.commit().await.unwrap();
    let response = match tokio::time::timeout(REQUEST_BOUND, &mut action).await {
        Ok(result) => result.unwrap(),
        Err(_) => {
            action.abort();
            let _ = action.await;
            panic!("the bodyless request did not finish after its identity wait");
        }
    };
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let bytes = to_bytes(
        response.into_body(),
        orch_api_contract::common::MAX_JSON_BYTES,
    )
    .await
    .unwrap();
    let problem: Json = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(problem["code"], "authz.denied");
    assert_eq!(state(&world, service).await, before);
}

#[tokio::test]
async fn an_authorized_service_can_delete_itself_without_revalidating_its_revoked_reply_credential()
{
    let world = World::new().await;
    let s = setup(&world).await;
    for (username, modern) in [
        ("ordinary-self-delete", false),
        ("scoped-self-delete", true),
    ] {
        let service = make(&world, &s.boss, s.acme, username, "Self deletion")
            .await
            .ok()
            .user
            .id;
        give(&world, &s.boss, s.web, service, Role::Developer).await;
        access::testing::add_entry(
            &mut *world.conn().await,
            &access::PageKey::new("node", s.acme, "live"),
            "any",
            "members.manage",
            ("user", Some(service), None),
            None,
            world.now(),
        )
        .await
        .unwrap();
        let secret = if modern {
            scoped(
                &world,
                service,
                vec![account_rule(service, "ServiceAccountDelete", "manage")],
                None,
            )
            .await
        } else {
            token(&world, &s.boss, service, &named("Ordinary self deletion"))
                .await
                .ok()
                .token
                .unwrap()
        };
        // The legacy '*' case establishes the unchanged ordinary self-service premise.
        empty_success(delete(&world, &secret, service).await);
        let after = state(&world, service).await;
        assert_eq!(after["status"], "suspended");
        assert_eq!(after["tokens"], 0);
        assert_eq!(after["roles"], 0);
        assert_eq!(after["entries"], 0);
        assert_eq!(after["deletions"], 1);
        assert_eq!(
            delete(&world, &secret, service).await.status,
            StatusCode::UNAUTHORIZED
        );
    }
}

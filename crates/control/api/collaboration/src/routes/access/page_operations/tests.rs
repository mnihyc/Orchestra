//! Native first-effect callback contracts: real pages, exact opaque errors and accepted tails.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

use crate::middleware::requests::ApiError;

use crate::places::make_org;

use crate::places::make_project;

use crate::places::person;

use crate::support::World;

use orch_access_contract::pages::PageChange;

use orch_access_contract::pages::Whole;

use orch_api_contract::access::PermissionPageGet;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::pages;

use orch_db::DbError;

use orch_types::Id;

use orch_types::Json;

use orch_types::Time;

use serde_json::json;

use std::sync::atomic::AtomicUsize;

use std::sync::atomic::Ordering;
async fn snapshot(world: &World, id: Id) -> Json {
    sqlx::query_scalar("SELECT jsonb_build_object('rows',(SELECT COALESCE(jsonb_agg(to_jsonb(r) ORDER BY permission,profile),'[]'::jsonb) FROM permission_rows r WHERE object_id=$1),'entries',(SELECT COALESCE(jsonb_agg(to_jsonb(e) ORDER BY id),'[]'::jsonb) FROM permission_entries e WHERE object_id=$1),'audit',(SELECT count(*) FROM audit_log WHERE object_id=$1))").bind(id).fetch_one(&world.db.pool).await.unwrap()
}

async fn page(world: &World, token: &str, id: Id) -> Json {
    let value = world
        .app
        .call(PermissionPageGet, &[&"nodes", &id])
        .token(token)
        .send()
        .await
        .ok();
    serde_json::to_value(value).unwrap()
}

#[cfg(any(test, feature = "test-support"))]
async fn refusal_keeps_opaque_trusted_or_untrusted_error_and_changes_no_page_audit_or_native_dto() {
    let world = World::new().await;
    let owner = person(&world, "page-fence-owner").await;
    let org = make_org(&world, &owner, "Fence office", "fence-office")
        .await
        .base
        .id;
    for trusted in [true, false] {
        let id = make_project(
            &world,
            &owner,
            org,
            if trusted {
                "Trusted boundary"
            } else {
                "Untrusted boundary"
            },
        )
        .await
        .base
        .id;
        let rows_before = snapshot(&world, id).await;
        let dto_before = page(&world, &owner.token, id).await;
        let calls = AtomicUsize::new(0);
        let original: ApiError = DbError::Retry.into();
        let original = if trusted {
            original
        } else {
            ApiError::new(original.code(), "public retry lookalike")
        }
        .with("opaque_boundary", "actual callback error")
        .retry_after(13); // Nondefault sentinel detects opaque transport metadata loss.
        let problem = original.problem.clone();
        assert_eq!(original.is_database_retry(), trusted);
        let mut tx = world.db.pool.begin().await.unwrap();
        let principal =
            Principal::load(&mut tx, Caller::session(owner.id()), Time::new(world.now()))
                .await
                .unwrap();
        let result = pages::change_fenced(
            &mut tx,
            &principal,
            pages::Target::Live(id),
            &PageChange {
                whole: Some(Whole::Private),
                ..PageChange::default()
            },
            Policy::default(),
            &orch_db::audit::Origin::default(),
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                Err::<(), ApiError>(original)
            },
        )
        .await;
        match result {
            Err(pages::ChangeFailure::BeforeEffect(error)) => {
                assert_eq!(
                    error.problem, problem,
                    "the opaque public envelope is unchanged"
                );
                assert_eq!(
                    error.is_database_retry(),
                    trusted,
                    "the private cause bit is unchanged"
                );
                let response = error.into_response_for("opaque-page-boundary");
                assert_eq!(
                    response.headers().get(http::header::RETRY_AFTER).unwrap(),
                    "13"
                );
            }
            other => panic!(
                "native valid preparation must reach its exact first-effect callback: {other:?}"
            ),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tx.commit().await.unwrap();
        assert_eq!(
            snapshot(&world, id).await,
            rows_before,
            "refused first effect changes no actual page or audit row"
        );
        assert_eq!(
            page(&world, &owner.token, id).await,
            dto_before,
            "ordinary native page DTO remains byte-equivalent as data"
        );
    }
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_refusal_keeps_opaque_trusted_or_untrusted_error_and_changes_no_page_audit_or_native_dto()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(self::refusal_keeps_opaque_trusted_or_untrusted_error_and_changes_no_page_audit_or_native_dto())
}

#[cfg(any(test, feature = "test-support"))]
async fn genuine_noop_never_calls_boundary_and_multirow_private_change_calls_only_once() {
    let world = World::new().await;
    let owner = person(&world, "page-tail-owner").await;
    let org = make_org(&world, &owner, "Tail office", "tail-office")
        .await
        .base
        .id;
    let id = make_project(&world, &owner, org, "Atomic native page")
        .await
        .base
        .id;
    let before = snapshot(&world, id).await;
    let calls = AtomicUsize::new(0);
    let mut tx = world.db.pool.begin().await.unwrap();
    let principal = Principal::load(&mut tx, Caller::session(owner.id()), Time::new(world.now()))
        .await
        .unwrap();
    let noop = pages::change_fenced(
        &mut tx,
        &principal,
        pages::Target::Live(id),
        &PageChange::default(),
        Policy::default(),
        &orch_db::audit::Origin::default(),
        || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<(), ApiError>(())
        },
    )
    .await
    .unwrap();
    assert!(!noop.changed);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    tx.commit().await.unwrap();
    assert_eq!(snapshot(&world, id).await, before);
    let mut tx = world.db.pool.begin().await.unwrap();
    let principal = Principal::load(&mut tx, Caller::session(owner.id()), Time::new(world.now()))
        .await
        .unwrap();
    let changed = pages::change_fenced(
        &mut tx,
        &principal,
        pages::Target::Live(id),
        &PageChange {
            whole: Some(Whole::Private),
            ..PageChange::default()
        },
        Policy::default(),
        &orch_db::audit::Origin::default(),
        || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<(), ApiError>(())
        },
    )
    .await
    .unwrap();
    assert!(changed.changed);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "later rows and audit are accepted atomic tails"
    );
    tx.commit().await.unwrap();
    let dto = page(&world, &owner.token, id).await;
    assert_eq!(dto["state"], json!("restricted"));
    assert_eq!(dto["revision"], json!(changed.revision));
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_genuine_noop_never_calls_boundary_and_multirow_private_change_calls_only_once()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::genuine_noop_never_calls_boundary_and_multirow_private_change_calls_only_once(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn an_empty_owners_object_cannot_relabel_a_whole_page_change_as_an_ownership_operation() {
    use crate::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_contract::access::PermissionPageUpdate;
    use orch_api_contract::common::ErrorCode;
    let world = World::new().await;
    let owner = person(&world, "variant-owner").await;
    let org = make_org(&world, &owner, "Variant office", "variant-office")
        .await
        .base
        .id;
    let native = make_project(&world, &owner, org, "Native empty owners premise")
        .await
        .base
        .id;
    let body = json!({"owners":{},"whole":"private"});
    let ordinary = world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &native])
        .token(&owner.token)
        .raw_body(&body)
        .send_bytes()
        .await;
    assert_eq!(
        ordinary.status,
        http::StatusCode::OK,
        "ordinary owner may make this native page change"
    );
    let target = make_project(&world, &owner, org, "Scoped actual page intent")
        .await
        .base
        .id;
    let before = snapshot(&world, target).await;
    let scope: KeyScope = serde_json::from_value(json!({"rules":[{"target":{"kind":"project","id":target,"reach":"self"},"allow":[{"operation":"PermissionPageUpdate.owners","actions":["manage"]}]}]})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    orch_db::queries::identity::create_api_token_scoped(
        &mut *world.conn().await,
        &orch_db::queries::identity::NewApiToken {
            id: orch_types::new_id(),
            user_id: owner.id(),
            name: "Actual page intent",
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
    let scoped = world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &target])
        .token(&secret)
        .raw_body(&body)
        .send_bytes()
        .await;
    assert_eq!(scoped.problem().code, ErrorCode::AuthzDenied);
    assert_eq!(
        snapshot(&world, target).await,
        before,
        "the real whole-page intent requires its own declared variant"
    );
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_an_empty_owners_object_cannot_relabel_a_whole_page_change_as_an_ownership_operation()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::an_empty_owners_object_cannot_relabel_a_whole_page_change_as_an_ownership_operation(),
    )
}

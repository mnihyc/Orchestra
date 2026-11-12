//! State isolation only; the actual HTTP/PG deadline cases are a separate integration target.
#![allow(clippy::unwrap_used, reason = "test assertions fail by panicking")]

use super::*;

use orch_api_contract::notifications::DestinationDelete;

use orch_db::queries::identity;

use orch_types::Clock;
#[cfg(any(test, feature = "test-support"))]
async fn provisional_disclosure_and_operands_cannot_replace_final_or_opened_state() {
    let db = orch_testkit::database::TestDb::fresh().await;
    let clock = orch_testkit::clock::manual();
    let app = crate::testing::TestApp::new(
        orch_db::Db::from_pool(db.pool.clone()),
        clock.clone(),
        Registry::new(),
    );
    let id = orch_types::new_id();
    let session = orch_types::new_id();
    let secret = authentication::new_token(authentication::ACCESS_TOKEN_PREFIX);
    let mut conn = db.pool.acquire().await.unwrap();
    identity::testing::insert_user(&mut conn, id, "opaque-original-authentication")
        .await
        .unwrap();
    identity::testing::insert_session(
        &mut conn,
        session,
        id,
        &authentication::token_hash(&secret),
        clock.now() + chrono::Duration::hours(1),
    )
    .await
    .unwrap();
    let user = Authed {
        as_admin: false,
        user: identity::user_by_id(&mut conn, id).await.unwrap().unwrap(),
        via: authentication::Via::Session {
            session_id: session,
        },
    };
    drop(conn);
    let ctx = Ctx::for_tool(
        app.app.clone(),
        Caller::User(Arc::new(user.clone())),
        DestinationDelete::ROUTE,
        orch_types::new_id(),
    );
    // Only actual state ownership is tested here; this tool context establishes
    // no HTTP or resource authority and invokes no business/native producer.
    ctx.publish_operation_data(OperationData::new(
        "earlier authenticated operands".to_owned(),
    ))
    .unwrap();
    ctx.retain_source("authenticated opened association".to_owned())
        .unwrap();
    let discovery = ctx.for_effect_discovery(user.clone(), clock.now());
    discovery
        .publish_operation_data(OperationData::new(
            "provisional protected payload".to_owned(),
        ))
        .unwrap();
    discovery.checkpoint_disclosure(Disclosure::Denied);
    assert_eq!(ctx.disclosure(), Disclosure::Full);
    assert_eq!(
        ctx.operation_data::<String>().unwrap().as_str(),
        "earlier authenticated operands"
    );
    assert_eq!(
        discovery.retained_source::<String>().unwrap().as_str(),
        "authenticated opened association"
    );
    assert_eq!(
        ctx.retained_source::<String>().unwrap().as_str(),
        "authenticated opened association"
    );
    let final_ctx = ctx.for_operation_checkpoint(user, clock.now());
    final_ctx
        .publish_operation_data(OperationData::new(
            "final freshly authorized operands".to_owned(),
        ))
        .unwrap();
    final_ctx.checkpoint_disclosure(Disclosure::Pending);
    assert_eq!(
        ctx.operation_data::<String>().unwrap().as_str(),
        "final freshly authorized operands"
    );
    assert_eq!(ctx.disclosure(), Disclosure::Pending);
    ctx.complete_disclosure(Disclosure::Full);
    let auxiliary = ctx.for_auxiliary_check();
    auxiliary.checkpoint_disclosure(Disclosure::Pending);
    auxiliary
        .publish_operation_data(OperationData::new("auxiliary audit operands".to_owned()))
        .unwrap();
    assert_eq!(ctx.disclosure(), Disclosure::Full);
    assert_eq!(
        ctx.operation_data::<String>().unwrap().as_str(),
        "final freshly authorized operands"
    );
    assert!(Arc::ptr_eq(&ctx.publication, &auxiliary.publication));
    assert_eq!(auxiliary.authenticated_user().unwrap().id(), id);
    assert!(!format!("{:?}", ctx.authenticated_user).contains("opaque-original-authentication"));
    let child = discovery.for_id(orch_types::new_id());
    assert!(
        child
            .optional_retained_source::<String>()
            .unwrap()
            .is_none()
    );
    assert!(child.operation_data::<String>().is_err());
    assert_eq!(
        child.authenticated_user().unwrap().id(),
        id,
        "original credential ceiling is not a previous-target proof"
    );
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_provisional_disclosure_and_operands_cannot_replace_final_or_opened_state()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::provisional_disclosure_and_operands_cannot_replace_final_or_opened_state(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn current_user_checkpoint_cannot_replace_original_authentication_ceiling() {
    let db = orch_testkit::database::TestDb::fresh().await;
    let clock = orch_testkit::clock::manual();
    let app = crate::testing::TestApp::new(
        orch_db::Db::from_pool(db.pool.clone()),
        clock.clone(),
        Registry::new(),
    );
    let id = orch_types::new_id();
    let mut conn = db.pool.acquire().await.unwrap();
    identity::testing::insert_user(&mut conn, id, "original-ceiling")
        .await
        .unwrap();
    let session = orch_types::new_id();
    let secret = authentication::new_token(authentication::ACCESS_TOKEN_PREFIX);
    identity::testing::insert_session(
        &mut conn,
        session,
        id,
        &authentication::token_hash(&secret),
        clock.now() + chrono::Duration::hours(1),
    )
    .await
    .unwrap();
    let user = Authed {
        as_admin: false,
        user: identity::user_by_id(&mut conn, id).await.unwrap().unwrap(),
        via: authentication::Via::Session {
            session_id: session,
        },
    };
    // No authentication is claimed by this context-only test.
    let ctx = Ctx::for_tool(
        app.app.clone(),
        Caller::User(Arc::new(user.clone())),
        DestinationDelete::ROUTE,
        orch_types::new_id(),
    );
    let mut later = user;
    later.as_admin = true;
    let current = ctx.for_operation_checkpoint(later, clock.now());
    assert!(current.user().unwrap().as_admin);
    assert!(!current.authenticated_user().unwrap().as_admin);
    assert_eq!(current.authenticated_user().unwrap().id(), id);
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_current_user_checkpoint_cannot_replace_original_authentication_ceiling()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::current_user_checkpoint_cannot_replace_original_authentication_ceiling(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn actual_http_identity_intersects_effective_original_and_current_admin_standing() {
    use crate::middleware::transaction_guard::{HttpIdentityMode, current_http_identity};
    let db = orch_testkit::database::TestDb::fresh().await;
    let clock = orch_testkit::clock::manual();
    let app = crate::testing::TestApp::new(
        orch_db::Db::from_pool(db.pool.clone()),
        clock.clone(),
        Registry::new(),
    );
    let id = orch_types::new_id();
    let session = orch_types::new_id();
    let secret = authentication::new_token(authentication::ACCESS_TOKEN_PREFIX);
    let mut conn = db.pool.acquire().await.unwrap();
    identity::testing::insert_user(&mut conn, id, "effective-admin-context")
        .await
        .unwrap();
    identity::testing::insert_session(
        &mut conn,
        session,
        id,
        &authentication::token_hash(&secret),
        clock.now() + chrono::Duration::hours(1),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE sessions SET as_admin=true WHERE id=$1")
        .bind(session)
        .execute(&mut *conn)
        .await
        .unwrap();
    // Standing revocation may leave the requested session mode bit set. Its
    // inverse also must not upgrade a request born without effective standing.
    for (original, current) in [(true, false), (false, true)] {
        sqlx::query("UPDATE users SET is_system_admin=$2 WHERE id=$1")
            .bind(id)
            .bind(original)
            .execute(&mut *conn)
            .await
            .unwrap();
        let user = Authed {
            as_admin: true,
            user: identity::user_by_id(&mut conn, id).await.unwrap().unwrap(),
            via: authentication::Via::Session {
                session_id: session,
            },
        };
        let mut ctx = Ctx::for_tool(
            app.app.clone(),
            Caller::User(Arc::new(user)),
            DestinationDelete::ROUTE,
            orch_types::new_id(),
        );
        ctx.surface = Some(HttpSurface::Browser);
        ctx.headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {secret}")).unwrap(),
        );
        sqlx::query("UPDATE users SET is_system_admin=$2 WHERE id=$1")
            .bind(id)
            .bind(current)
            .execute(&mut *conn)
            .await
            .unwrap();
        let actual = current_http_identity(&ctx, &mut conn, HttpIdentityMode::Read)
            .await
            .unwrap();
        assert!(!actual.caller.as_admin());
        assert!(!actual.user.as_admin);
        assert_eq!(actual.user.user.is_system_admin, current);
        assert!(actual.expires_at.is_some());
    }
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_actual_http_identity_intersects_effective_original_and_current_admin_standing()
-> ::futures::future::LocalBoxFuture<'static, ()> {
    ::std::boxed::Box::pin(
        self::actual_http_identity_intersects_effective_original_and_current_admin_standing(),
    )
}

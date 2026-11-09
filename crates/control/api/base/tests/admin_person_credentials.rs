//! A person's current credentials are inspectable and revocable through native system administration.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod admin_support;
#[path = "admin_person_credentials/permission_rules.rs"]
mod permission_rules;
mod places;
mod support;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::{
    admin::{
        AdminTokenListQuery, AdminUserSessionDelete, AdminUserSessionList, AdminUserSessionsDelete,
        AdminUserTokenActivityList, AdminUserTokenDelete, AdminUserTokenList,
    },
    auth::{ApiTokenActivityQuery, MeGet},
    common::{ErrorCode, PageQuery},
};
use orch_api_core::{
    middleware::authentication::{ACCESS_TOKEN_PREFIX, API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::identity::{self, NewApiToken, NewSession};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;
async fn session(world: &World, owner: Id) -> (Id, String) {
    let id = new_id();
    let bearer = new_token(ACCESS_TOKEN_PREFIX);
    let cookie = new_token(ACCESS_TOKEN_PREFIX);
    let mut conn = world.conn().await;
    identity::create_session(
        &mut conn,
        &NewSession {
            id,
            user_id: owner,
            refresh_hash: &token_hash(&cookie),
            user_agent: "Native session control",
            ip: Some("192.0.2.1".parse().unwrap()),
            at: world.now(),
            expires_at: world.now() + chrono::Duration::hours(1),
        },
    )
    .await
    .unwrap();
    identity::add_access_token(
        &mut conn,
        &token_hash(&bearer),
        id,
        world.now() + chrono::Duration::hours(1),
    )
    .await
    .unwrap();
    (id, bearer)
}
async fn key(world: &World, owner: Id, rules: Option<Vec<Json>>) -> (Id, String) {
    let policy = rules.map(|rules| {
        let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        serde_json::to_value(scope).unwrap()
    });
    let id = new_id();
    let bearer = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: owner,
            name: "Person credential control",
            token_hash: &token_hash(&bearer),
            scopes: &if policy.is_some() {
                vec![]
            } else {
                vec!["doc.view".into()]
            },
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        policy.as_ref(),
    )
    .await
    .unwrap();
    (id, bearer)
}
fn rule(kind: &str, id: Id, operation: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],"allow":[{"operation":operation,"actions":actions}]})
}
fn value(reply: RawReply) -> Json {
    assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.raw));
    serde_json::from_slice(&reply.raw).unwrap()
}
#[tokio::test]
async fn sessions_keep_current_metadata_and_a_single_revoker_cannot_cross_the_person_boundary() {
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    let (id, bearer) = session(&world, c.ada.id()).await;
    let all = value(
        world
            .app
            .call(AdminUserSessionList, &[&c.ada.id()])
            .token(&c.root.token)
            .query(&PageQuery::default())
            .send_bytes()
            .await,
    );
    let row = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == json!(id))
        .unwrap();
    assert_eq!(row["user_agent"], "Native session control");
    assert_eq!(row["ip"], "192.0.2.1");
    assert_eq!(row["current"], false);
    assert_eq!(row["admin_access"], true);
    assert!(!all.to_string().contains("refresh_hash"));
    assert!(!all.to_string().contains(&bearer));
    let (_, reader) = key(
        &world,
        c.root.id(),
        Some(vec![
            rule("session", id, "AdminUserSessionList", &["read"]),
            rule("session", id, "AdminUserSessionDelete", &["manage"]),
        ]),
    )
    .await;
    let shown = value(
        world
            .app
            .call(AdminUserSessionList, &[&c.ada.id()])
            .token(&reader)
            .query(&PageQuery {
                limit: Some(1),
                cursor: None,
            })
            .send_bytes()
            .await,
    );
    assert_eq!(shown["items"].as_array().unwrap().len(), 1);
    assert_eq!(shown["items"][0]["id"], json!(id));
    assert!(shown["next_cursor"].is_null());
    assert_eq!(
        world
            .app
            .call(AdminUserSessionDelete, &[&c.boss.id(), &id])
            .token(&c.root.token)
            .send_bytes()
            .await
            .status,
        404
    );
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(&bearer)
            .send_bytes()
            .await
            .status,
        200
    );
    assert_eq!(
        world
            .app
            .call(AdminUserSessionDelete, &[&c.ada.id(), &id])
            .token(&reader)
            .send_bytes()
            .await
            .status,
        204
    );
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(&bearer)
            .send_bytes()
            .await
            .status,
        401
    );
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(&c.ada.token)
            .send_bytes()
            .await
            .status,
        200
    );
    let rows = admin_support::lines_of(&world, "auth.session_revoked").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].actor_id, Some(c.root.id()));
    assert_eq!(rows[0].object_id, Some(c.ada.id()));
    assert_eq!(rows[0].details["sessions"], json!([id]));
}
#[tokio::test]
async fn all_session_revocation_binds_the_whole_set_and_never_silently_filters_it() {
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    let (id, one) = session(&world, c.ada.id()).await;
    let (_, two) = session(&world, c.ada.id()).await;
    let root = json!({"target":{"kind":"account","id":c.ada.id(),"reach":"self"},"resources":["session"],"allow":[{"operation":"AdminUserSessionsDelete","actions":["manage"]}]});
    let (_, partial) = key(
        &world,
        c.root.id(),
        Some(vec![
            root,
            rule("session", id, "AdminUserSessionsDelete", &["manage"]),
        ]),
    )
    .await;
    assert_eq!(
        world
            .app
            .call(AdminUserSessionsDelete, &[&c.ada.id()])
            .token(&partial)
            .send_bytes()
            .await
            .status,
        403
    );
    for token in [&one, &two, &c.ada.token] {
        assert_eq!(
            world
                .app
                .call(MeGet, &[])
                .token(token)
                .send_bytes()
                .await
                .status,
            200
        );
    }
    assert!(
        admin_support::lines_of(&world, "auth.session_revoked")
            .await
            .is_empty()
    );
    assert_eq!(
        world
            .app
            .call(AdminUserSessionsDelete, &[&c.ada.id()])
            .token(&c.root.token)
            .send_bytes()
            .await
            .status,
        204
    );
    for token in [&one, &two, &c.ada.token] {
        assert_eq!(
            world
                .app
                .call(MeGet, &[])
                .token(token)
                .send_bytes()
                .await
                .status,
            401
        );
    }
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(&c.boss.token)
            .send_bytes()
            .await
            .status,
        200
    );
    let rows = admin_support::lines_of(&world, "auth.session_revoked").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].details["sessions_ended"], 3);
    assert_eq!(rows[0].details["all"], true);
}
#[tokio::test]
async fn per_person_tokens_use_their_own_selection_and_retained_native_activity_after_revoke() {
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    let (id, bearer) = key(&world, c.ada.id(), None).await;
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(&bearer)
            .send_bytes()
            .await
            .status,
        200
    );
    let page = value(
        world
            .app
            .call(AdminUserTokenList, &[&c.ada.id()])
            .token(&c.root.token)
            .query(&AdminTokenListQuery::default())
            .send_bytes()
            .await,
    );
    assert_eq!(page["items"][0]["info"]["id"], json!(id));
    assert_eq!(page["items"][0]["owner"]["id"], json!(c.ada.id()));
    let (_, reader) = key(
        &world,
        c.root.id(),
        Some(vec![
            rule("key", id, "AdminUserTokenList", &["read"]),
            rule("key", id, "AdminUserTokenActivityList", &["read"]),
            rule("key", id, "AdminUserTokenDelete", &["manage"]),
        ]),
    )
    .await;
    let page = value(
        world
            .app
            .call(AdminUserTokenList, &[&c.ada.id()])
            .token(&reader)
            .query(&AdminTokenListQuery::default())
            .send_bytes()
            .await,
    );
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let before = admin_support::lines_of(&world, "admin.opened").await.len();
    assert_eq!(
        world
            .app
            .call(AdminUserTokenActivityList, &[&c.boss.id(), &id])
            .token(&c.root.token)
            .query(&ApiTokenActivityQuery::default())
            .send_bytes()
            .await
            .status,
        404
    );
    assert_eq!(
        admin_support::lines_of(&world, "admin.opened").await.len(),
        before
    );
    let activity = value(
        world
            .app
            .call(AdminUserTokenActivityList, &[&c.ada.id(), &id])
            .token(&reader)
            .query(&ApiTokenActivityQuery::default())
            .send_bytes()
            .await,
    );
    assert!(
        activity["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["operation"] == "MeGet"
                && row["status"] == 200
                && row["admin_access"] == true)
    );
    assert!(!activity.to_string().contains(&bearer));
    assert_eq!(
        world
            .app
            .call(AdminUserTokenDelete, &[&c.boss.id(), &id])
            .token(&c.root.token)
            .send_bytes()
            .await
            .status,
        404
    );
    assert_eq!(
        world
            .app
            .call(AdminUserTokenDelete, &[&c.ada.id(), &id])
            .token(&reader)
            .send_bytes()
            .await
            .status,
        204
    );
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(&bearer)
            .send_bytes()
            .await
            .status,
        401
    );
    let retained = value(
        world
            .app
            .call(AdminUserTokenActivityList, &[&c.ada.id(), &id])
            .token(&reader)
            .query(&ApiTokenActivityQuery::default())
            .send_bytes()
            .await,
    );
    assert!(
        retained["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["operation"] == "MeGet" && row["status"] == 200)
    );
}
#[tokio::test]
async fn person_credential_metadata_needs_current_admin_mode_and_self_revoke_all_keeps_its_receipt()
{
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    for token in [&c.boss.token, &c.ada.token] {
        assert_eq!(
            world
                .app
                .call(AdminUserSessionList, &[&c.ada.id()])
                .token(token)
                .query(&PageQuery::default())
                .send_bytes()
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied
        );
    }
    places::admin_mode(&world, &c.root, false).await;
    assert_eq!(
        world
            .app
            .call(AdminUserTokenList, &[&c.ada.id()])
            .token(&c.root.token)
            .query(&AdminTokenListQuery::default())
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    places::admin_mode(&world, &c.root, true).await;
    assert_eq!(
        world
            .app
            .call(AdminUserSessionsDelete, &[&c.root.id()])
            .token(&c.root.token)
            .send_bytes()
            .await
            .status,
        204
    );
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(&c.root.token)
            .send_bytes()
            .await
            .status,
        401
    );
    let lines = admin_support::lines_of(&world, "auth.session_revoked").await;
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].object_id, Some(c.root.id()));
}

#[tokio::test]
async fn person_session_and_activity_delivery_keep_current_system_standing_and_browser_mode() {
    for activity in [false, true] {
        let world = World::new().await;
        let c = admin_support::cast(&world).await;
        let (id, bearer) = key(&world, c.ada.id(), None).await;
        assert_eq!(
            world
                .app
                .call(MeGet, &[])
                .token(&bearer)
                .send_bytes()
                .await
                .status,
            200
        );
        let response = if activity {
            world
                .app
                .call(AdminUserTokenActivityList, &[&c.ada.id(), &id])
                .token(&c.root.token)
                .query(&ApiTokenActivityQuery::default())
                .send_response()
                .await
        } else {
            world
                .app
                .call(AdminUserSessionList, &[&c.ada.id()])
                .token(&c.root.token)
                .query(&PageQuery::default())
                .send_response()
                .await
        };
        assert_eq!(response.status(), 200);
        if activity {
            sqlx::query("UPDATE sessions SET as_admin=false WHERE user_id=$1")
                .bind(c.root.id())
                .execute(&world.db.pool)
                .await
                .unwrap();
        } else {
            sqlx::query("UPDATE users SET is_system_admin=false WHERE id=$1")
                .bind(c.root.id())
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        assert!(
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .is_err()
        );
        assert_eq!(
            world
                .app
                .call(MeGet, &[])
                .token(&c.ada.token)
                .send_bytes()
                .await
                .status,
            200
        );
        assert!(
            admin_support::lines_of(&world, "auth.session_revoked")
                .await
                .is_empty()
        );
    }
}

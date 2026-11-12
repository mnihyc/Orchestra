//! Actual system credential metadata, scope selection and audited revocation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod admin_support;
mod places;
mod support;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::{
    admin::{AdminTokenDelete, AdminTokenList, AdminTokenListQuery},
    auth::MeGet,
    common::ErrorCode,
};
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;

async fn token(
    world: &World,
    owner: Id,
    name: &str,
    policy: Option<Json>,
    expired: bool,
    node: Option<Id>,
) -> (Id, String) {
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: owner,
            name,
            token_hash: &token_hash(&secret),
            scopes: &if policy.is_some() {
                vec![]
            } else {
                vec!["doc.view".into()]
            },
            node_id: node,
            at: world.now() - chrono::Duration::days(2),
            expires_at: Some(world.now() + chrono::Duration::hours(if expired { -1 } else { 1 })),
        },
        policy.as_ref(),
    )
    .await
    .unwrap();
    (id, secret)
}
fn policy(id: Id, operation: &str, actions: &[&str]) -> Json {
    let scope:KeyScope=serde_json::from_value(json!({"rules":[{
        "target":{"kind":"key","id":id,"reach":"self"},"resources":["key"],"allow":[{"operation":operation,"actions":actions}]
    }]})).unwrap();
    assert!(scope.validate().is_empty());
    serde_json::to_value(scope).unwrap()
}
fn value(reply: RawReply) -> Json {
    assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.raw));
    serde_json::from_slice(&reply.raw).unwrap()
}
#[tokio::test]
async fn system_tokens_are_bounded_include_actual_owners_and_expired_metadata_and_never_secrets() {
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    let (one, secret) = token(
        &world,
        c.ada.id(),
        "Personal automation",
        None,
        false,
        Some(c.acme),
    )
    .await;
    let (two, _) = token(
        &world,
        c.boss.id(),
        "Expired automation",
        Some(policy(one, "AdminTokenList", &["read"])),
        true,
        None,
    )
    .await;
    let mut query = AdminTokenListQuery {
        limit: Some(1),
        ..Default::default()
    };
    let mut ids = Vec::new();
    loop {
        let page = value(
            world
                .app
                .call(AdminTokenList, &[])
                .token(&c.root.token)
                .query(&query)
                .send_bytes()
                .await,
        );
        let rows = page["items"].as_array().unwrap();
        assert!(rows.len() <= 1);
        for row in rows {
            ids.push(serde_json::from_value::<Id>(row["info"]["id"].clone()).unwrap());
            assert_eq!(row["admin_access"], true);
        }
        let text = page.to_string();
        assert!(!text.contains(&secret));
        assert!(!text.contains("token_hash"));
        assert!(page["items"][0]["info"]["token"].is_null());
        match page["next_cursor"].as_str() {
            Some(cursor) => query.cursor = Some(cursor.into()),
            None => break,
        }
    }
    ids.sort();
    let mut expected = vec![one, two];
    expected.sort();
    assert_eq!(ids, expected);
    let query = AdminTokenListQuery {
        owner_id: Some(c.ada.id()),
        ..Default::default()
    };
    let page = value(
        world
            .app
            .call(AdminTokenList, &[])
            .token(&c.root.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["owner"]["id"], json!(c.ada.id()));
    assert_eq!(page["items"][0]["info"]["id"], json!(one));
    assert_eq!(page["items"][0]["info"]["node"]["name"], "Acme");
    assert_eq!(
        admin_support::lines_of(&world, "admin.opened").await.len(),
        3
    );
}
#[tokio::test]
async fn system_token_inventory_requires_current_system_standing_and_the_explicit_browser_mode() {
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    for by in [&c.boss, &c.ada] {
        assert_eq!(
            world
                .app
                .call(AdminTokenList, &[])
                .token(&by.token)
                .query(&AdminTokenListQuery::default())
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
            .call(AdminTokenList, &[])
            .token(&c.root.token)
            .query(&AdminTokenListQuery::default())
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let (_, legacy) = token(&world, c.root.id(), "Legacy admin", None, false, None).await;
    assert_eq!(
        world
            .app
            .call(AdminTokenList, &[])
            .token(&legacy)
            .query(&AdminTokenListQuery::default())
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthSessionRequired
    );
    assert!(
        admin_support::lines_of(&world, "admin.opened")
            .await
            .is_empty()
    );
}
#[tokio::test]
async fn scoped_system_inventory_selects_keys_before_paging_and_protects_optional_node_names() {
    if std::env::var_os("ORCH_TEST_NATIVE_OPERATIONS").is_some() {
        support::Statements::now();
    }
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    for i in 0..4 {
        token(
            &world,
            c.boss.id(),
            &format!("Other key {i}"),
            None,
            false,
            None,
        )
        .await;
    }
    let (id, _) = token(
        &world,
        c.ada.id(),
        "Selected key",
        None,
        false,
        Some(c.acme),
    )
    .await;
    let (_, scoped) = token(
        &world,
        c.root.id(),
        "Inventory reader",
        Some(policy(id, "AdminTokenList", &["read"])),
        false,
        None,
    )
    .await;
    let page = value(
        world
            .app
            .call(AdminTokenList, &[])
            .token(&scoped)
            .query(&AdminTokenListQuery {
                limit: Some(1),
                ..Default::default()
            })
            .send_bytes()
            .await,
    );
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["info"]["id"], json!(id));
    assert!(page["next_cursor"].is_null());
    assert!(page["items"][0]["info"]["node"]["name"].is_null());
    assert_eq!(page["items"][0]["info"]["node"]["redacted"], true);
    let opened = admin_support::lines_of(&world, "admin.opened").await;
    assert_eq!(
        opened.len(),
        1,
        "the successful scoped read retains its audit"
    );
    sqlx::query("UPDATE users SET is_system_admin=false WHERE id=$1")
        .bind(c.root.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        world
            .app
            .call(AdminTokenList, &[])
            .token(&scoped)
            .query(&AdminTokenListQuery::default())
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        admin_support::lines_of(&world, "admin.opened").await.len(),
        1,
        "a refused current administrator cannot record another opening"
    );
}
#[tokio::test]
async fn targeted_manage_revokes_the_real_foreign_key_once_and_audits_its_owner() {
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    let (id, secret) = token(&world, c.ada.id(), "Revoke me", None, false, None).await;
    let (other, _) = token(&world, c.boss.id(), "Keep me", None, false, None).await;
    let (_, admin) = token(
        &world,
        c.root.id(),
        "Revoker",
        Some(policy(id, "AdminTokenDelete", &["manage"])),
        false,
        None,
    )
    .await;
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(&secret)
            .send_bytes()
            .await
            .status,
        200
    );
    assert_eq!(
        world
            .app
            .call(AdminTokenDelete, &[&other])
            .token(&admin)
            .send_bytes()
            .await
            .status,
        403
    );
    assert_eq!(
        world
            .app
            .call(AdminTokenDelete, &[&id])
            .token(&admin)
            .send_bytes()
            .await
            .status,
        204
    );
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(&secret)
            .send_bytes()
            .await
            .status,
        401
    );
    assert!(
        identity::api_token_listed(&mut *world.conn().await, other, c.boss.id())
            .await
            .unwrap()
            .is_some()
    );
    let rows = admin_support::lines_of(&world, "auth.token_revoked").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].actor_id, Some(c.root.id()));
    assert_eq!(rows[0].object_id, Some(c.ada.id()));
    assert_eq!(rows[0].details["token"], json!(id));
    assert_eq!(
        world
            .app
            .call(AdminTokenDelete, &[&id])
            .token(&admin)
            .send_bytes()
            .await
            .status,
        404
    );
    assert_eq!(
        admin_support::lines_of(&world, "auth.token_revoked")
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn inventory_uses_real_service_ownership_and_never_a_humans_legacy_node_as_placement() {
    if std::env::var_os("ORCH_TEST_NATIVE_OPERATIONS").is_some() {
        support::Statements::now();
    }
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    let service = new_id();
    identity::create_service_account(
        &mut *world.conn().await,
        service,
        c.acme,
        "system_inventory_robot",
        "Inventory robot",
        c.boss.id(),
        world.now(),
    )
    .await
    .unwrap();
    let (service_key, _) = token(&world, service, "Actual service owner", None, false, None).await;
    let (human_key, _) = token(
        &world,
        c.boss.id(),
        "Human with a legacy restriction",
        None,
        false,
        Some(c.acme),
    )
    .await;
    let scope:KeyScope=serde_json::from_value(json!({"rules":[{
        "target":{"kind":"org","id":c.acme,"reach":"subtree"},"resources":["key"],"allow":[{"operation":"AdminTokenList","actions":["read"]}]
    }]})).unwrap();
    assert!(scope.validate().is_empty());
    let (_, reader) = token(
        &world,
        c.root.id(),
        "Service subtree reader",
        Some(serde_json::to_value(scope).unwrap()),
        false,
        None,
    )
    .await;
    let page = value(
        world
            .app
            .call(AdminTokenList, &[])
            .token(&reader)
            .query(&AdminTokenListQuery::default())
            .send_bytes()
            .await,
    );
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["info"]["id"], json!(service_key));
    assert_ne!(page["items"][0]["info"]["id"], json!(human_key));
    assert_eq!(page["items"][0]["owner"]["id"], json!(service));
    assert_eq!(page["items"][0]["owner"]["kind"], "service");
}

#[tokio::test]
async fn token_metadata_preserves_conjunctive_policy_and_rejects_cursor_owner_substitution() {
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    let (id, _) = token(
        &world,
        c.ada.id(),
        "First metadata record",
        None,
        false,
        None,
    )
    .await;
    let mut scoped = policy(id, "AdminTokenList", &["read"]);
    scoped["constraints"] = json!([[scoped["rules"][0].clone()]]);
    let (other, _) = token(
        &world,
        c.ada.id(),
        "Constrained metadata record",
        Some(scoped.clone()),
        false,
        None,
    )
    .await;
    let page = value(
        world
            .app
            .call(AdminTokenList, &[])
            .token(&c.root.token)
            .query(&AdminTokenListQuery {
                owner_id: Some(c.ada.id()),
                limit: Some(1),
                cursor: None,
            })
            .send_bytes()
            .await,
    );
    let cursor = page["next_cursor"].as_str().unwrap().to_owned();
    let changed = world
        .app
        .call(AdminTokenList, &[])
        .token(&c.root.token)
        .query(&AdminTokenListQuery {
            owner_id: Some(c.boss.id()),
            limit: Some(1),
            cursor: Some(cursor),
        })
        .send_bytes()
        .await;
    assert_eq!(changed.problem().code, ErrorCode::RequestInvalid);
    let all = value(
        world
            .app
            .call(AdminTokenList, &[])
            .token(&c.root.token)
            .query(&AdminTokenListQuery {
                owner_id: Some(c.ada.id()),
                ..Default::default()
            })
            .send_bytes()
            .await,
    );
    let row = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["info"]["id"] == json!(other))
        .unwrap();
    assert_eq!(row["info"]["rules"], scoped["rules"]);
    assert_eq!(row["info"]["constraints"], scoped["constraints"]);
}

#[tokio::test]
async fn browser_admin_mode_and_current_key_scope_are_rechecked_at_real_metadata_delivery() {
    if std::env::var_os("ORCH_TEST_NATIVE_OPERATIONS").is_some() {
        support::Statements::now();
    }
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    let (id, _) = token(
        &world,
        c.ada.id(),
        "Native delivered metadata",
        None,
        false,
        None,
    )
    .await;
    // send_response completes actual selection/audit and headers, leaving the real checked
    // response body pending. No data frame has been exposed at this boundary.
    let response = world
        .app
        .call(AdminTokenList, &[])
        .token(&c.root.token)
        .query(&AdminTokenListQuery::default())
        .send_response()
        .await;
    assert_eq!(response.status(), 200);
    sqlx::query("UPDATE sessions SET as_admin=false WHERE user_id=$1")
        .bind(c.root.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert!(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .is_err()
    );
    places::admin_mode(&world, &c.root, true).await;
    let (reader_id, reader) = token(
        &world,
        c.root.id(),
        "Current scope reader",
        Some(policy(id, "AdminTokenList", &["read"])),
        false,
        None,
    )
    .await;
    let response = world
        .app
        .call(AdminTokenList, &[])
        .token(&reader)
        .query(&AdminTokenListQuery::default())
        .send_response()
        .await;
    assert_eq!(response.status(), 200);
    let denied: KeyScope = serde_json::from_value(json!({"rules":[]})).unwrap();
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(reader_id)
        .bind(serde_json::to_value(denied).unwrap())
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert!(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .is_err()
    );
    assert!(
        identity::api_token_listed(&mut *world.conn().await, id, c.ada.id())
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn metadata_delivery_keeps_the_original_raw_deadline_when_the_browser_access_token_extends() {
    let world = World::new().await;
    let c = admin_support::cast(&world).await;
    token(&world, c.ada.id(), "Lifetime metadata", None, false, None).await;
    sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE token_hash=$1")
        .bind(token_hash(&c.root.token))
        .bind(world.now() + chrono::Duration::seconds(5))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let response = world
        .app
        .call(AdminTokenList, &[])
        .token(&c.root.token)
        .query(&AdminTokenListQuery::default())
        .send_response()
        .await;
    assert_eq!(response.status(), 200);
    sqlx::query("UPDATE access_tokens SET expires_at=$2 WHERE token_hash=$1")
        .bind(token_hash(&c.root.token))
        .bind(world.now() + chrono::Duration::hours(1))
        .execute(&world.db.pool)
        .await
        .unwrap();
    world.advance(chrono::Duration::seconds(6));
    assert!(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .is_err()
    );
}

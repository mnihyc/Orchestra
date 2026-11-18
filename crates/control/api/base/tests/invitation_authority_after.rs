//! After-only checks for the private issuer column; baseline has no such column.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

mod chat_world;
mod places;
mod support;

use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::access::Role;
use orch_api_contract::users::{
    InvitationCreate, InvitationCreateBody, InvitationKind, InvitationLookup,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity;
use orch_types::{Id, Json, new_id};
use serde_json::json;

fn body(project: Id) -> InvitationCreateBody {
    InvitationCreateBody {
        kind: InvitationKind::Node,
        org_id: None,
        node_id: Some(project),
        chat_id: None,
        role: Some(Role::Viewer),
        permissions: None,
        email: None,
        max_uses: Some(3),
        expires_in_minutes: Some(60),
        note: None,
    }
}
fn checked(reply: RawReply, expected: StatusCode) -> Json {
    let value: Json = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(reply.status, expected, "code={}", value["code"]);
    value
}
async fn issuer(room: &chat_world::Room) -> (Id, String) {
    let policy:KeyScope=serde_json::from_value(json!({"rules":[
        {"target":{"kind":"project","id":room.project,"reach":"self"},"resources":["invitation"],"allow":[{"operation":"InvitationCreate","actions":["manage","read"]}]},
        {"target":{"kind":"project","id":room.project,"reach":"self"},"resources":["project"],"allow":[{"operation":"InvitationCreate","actions":["read"]}]},
        {"target":{"kind":"org","id":room.root,"reach":"self"},"resources":["org"],"allow":[{"operation":"InvitationCreate","actions":["read"]}]}
    ]})).unwrap();
    assert!(policy.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *room.world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: room.ada.id(),
            name: "Private issuer metadata check",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: room.world.now(),
            expires_at: Some(room.world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(policy).unwrap()),
    )
    .await
    .unwrap();
    (id, secret)
}
async fn created(room: &chat_world::Room, secret: &str) -> (Id, String) {
    let reply = checked(
        room.world
            .app
            .call(InvitationCreate, &[])
            .token(secret)
            .body(&body(room.project))
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert!(
        reply["invitation"].is_object(),
        "this control requires ordinary Full creation"
    );
    (
        serde_json::from_value(reply["invitation"]["id"].clone()).unwrap(),
        reply["token"].as_str().unwrap().into(),
    )
}

#[tokio::test]
async fn recognized_wrong_selection_user_or_malformed_origin_never_falls_back_to_a_session() {
    let room = chat_world::Room::new().await;
    let (_, key) = issuer(&room).await;
    let (id, token) = created(&room, &key).await;
    let saved: Json = sqlx::query_scalar("SELECT issuer_origin FROM invitations WHERE id=$1")
        .bind(id)
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap();
    assert!(saved.is_object());
    for change in ["selection", "user", "shape"] {
        checked(
            room.world
                .app
                .call(InvitationLookup, &[&token])
                .send_bytes()
                .await,
            StatusCode::OK,
        );
        let mut invalid = saved.clone();
        match change {
            "selection" => invalid["operation"] = json!("InvitationDelete"),
            "user" => invalid["caller"]["user"] = json!(room.bea.id()),
            _ => invalid = json!({"unexpected":true}),
        }
        sqlx::query("UPDATE invitations SET issuer_origin=$2 WHERE id=$1")
            .bind(id)
            .bind(invalid)
            .execute(&room.world.db.pool)
            .await
            .unwrap();
        let denied = checked(
            room.world
                .app
                .call(InvitationLookup, &[&token])
                .send_bytes()
                .await,
            StatusCode::GONE,
        );
        assert_eq!(denied["reason"], "invitation_issuer_authority_lost");
        sqlx::query("UPDATE invitations SET issuer_origin=$2 WHERE id=$1")
            .bind(id)
            .bind(saved.clone())
            .execute(&room.world.db.pool)
            .await
            .unwrap();
    }
    let (native, token) = created(&room, &room.ada.token).await;
    let legacy: bool =
        sqlx::query_scalar("SELECT issuer_origin IS NULL FROM invitations WHERE id=$1")
            .bind(native)
            .fetch_one(&room.world.db.pool)
            .await
            .unwrap();
    assert!(
        legacy,
        "ordinary browser offers retain their existing captured-mode lifetime"
    );
    checked(
        room.world
            .app
            .call(InvitationLookup, &[&token])
            .send_bytes()
            .await,
        StatusCode::OK,
    );
}

#[tokio::test]
async fn an_active_owner_key_withdraws_an_outstanding_offer_when_its_create_scope_is_narrowed() {
    let room = chat_world::Room::new().await;
    let (key, secret) = issuer(&room).await;
    let (id, token) = created(&room, &secret).await;
    checked(
        room.world
            .app
            .call(InvitationLookup, &[&token])
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(key)
        .bind(serde_json::to_value(KeyScope::default()).unwrap())
        .execute(&room.world.db.pool)
        .await
        .unwrap();
    let active: bool = sqlx::query_scalar(
        "SELECT revoked_at IS NULL AND expires_at>$2 FROM api_tokens WHERE id=$1",
    )
    .bind(key)
    .bind(room.world.now())
    .fetch_one(&room.world.db.pool)
    .await
    .unwrap();
    assert!(active, "only the actual operation ceiling changed");
    let denied = checked(
        room.world
            .app
            .call(InvitationLookup, &[&token])
            .send_bytes()
            .await,
        StatusCode::GONE,
    );
    assert_eq!(denied["reason"], "invitation_issuer_authority_lost");
    let used: i32 = sqlx::query_scalar("SELECT uses FROM invitations WHERE id=$1")
        .bind(id)
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap();
    assert_eq!(used, 0);
}

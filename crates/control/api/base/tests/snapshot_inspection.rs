//! Native snapshot inspection is carried to the renderer, transport and audit.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod desk;
mod places;
mod support;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::{
    documents::{DocumentDelete, DocumentGet, DocumentVersionList, DocumentVersionListQuery},
    nodes::DeletedInspectionQuery,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::new_id;
use serde_json::json;
#[tokio::test]
async fn stored_snapshot_inspection_keeps_its_exact_retained_scope_and_audits_only_success() {
    let desk = desk::Desk::new().await;
    let id = desk
        .make(&desk.ada, "Retained native snapshot")
        .await
        .base
        .id;
    desk.world
        .app
        .call(DocumentDelete, &[&id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let key = new_token(API_TOKEN_PREFIX);
    let scope: KeyScope = serde_json::from_value(json!({"rules":[{
        "target":{"kind":"document","id":id,"reach":"self"},"allow":[
            {"operation":"DocumentGet.inspect_deleted","actions":["manage"]},
            {"operation":"DocumentVersionList.inspect_deleted","actions":["manage"]}
        ]
    }]}))
    .unwrap();
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: desk.ada.id(),
            name: "Retained snapshot",
            token_hash: &token_hash(&key),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: Some(desk.world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    let denied = desk
        .world
        .app
        .call(DocumentGet, &[&id])
        .token(&key)
        .query(&DeletedInspectionQuery::default())
        .send_bytes()
        .await;
    assert!(denied.status.as_u16() >= 400);
    let before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='deleted.inspected' AND object_id=$1",
    )
    .bind(id)
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap();
    assert_eq!(before, 0);
    let result = desk
        .world
        .app
        .call(DocumentGet, &[&id])
        .token(&key)
        .query(&DeletedInspectionQuery {
            inspect_deleted: Some(true),
        })
        .send_bytes()
        .await;
    assert_eq!(
        result.status,
        200,
        "{}",
        String::from_utf8_lossy(&result.raw)
    );
    assert!(result.headers.contains_key("orch-inspection"));
    let object: serde_json::Value = serde_json::from_slice(&result.raw).unwrap();
    assert_eq!(object["id"], json!(id));
    let history = desk
        .world
        .app
        .call(DocumentVersionList, &[&id])
        .token(&key)
        .query(&DocumentVersionListQuery {
            inspect_deleted: Some(true),
            ..Default::default()
        })
        .send_bytes()
        .await;
    assert_eq!(
        history.status,
        200,
        "{}",
        String::from_utf8_lossy(&history.raw)
    );
    assert!(history.headers.contains_key("orch-inspection"));
    let still_deleted: bool =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM documents WHERE id=$1")
            .bind(id)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
    assert!(still_deleted);
    let after:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='deleted.inspected' AND object_id=$1 AND actor_id=$2")
        .bind(id).bind(desk.ada.id()).fetch_one(&desk.world.db.pool).await.unwrap();
    assert!(after >= 1);
}

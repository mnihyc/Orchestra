//! Document list scopes precede both native windows and protect live reference names.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod desk;
mod places;
mod support;

use crate::support::FullPermissionPageReply;
use desk::{Desk, body};
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::access::{PermissionPageGet, PermissionPageUpdate};
use orch_api_contract::documents::{
    DeletedFilter, DocumentDelete, DocumentGet, NewDocumentAccess, NodeDocumentList,
    NodeDocumentListQuery,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use serde_json::json;

struct Key {
    id: Id,
    secret: String,
}

fn rule(operation: &str, kind: &str, id: Id, action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},
        "allow":[{"operation":operation,"actions":[action]}]})
}

async fn key(desk: &Desk, user: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let key = Key {
        id: new_id(),
        secret: new_token(API_TOKEN_PREFIX),
    };
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: key.id,
            user_id: user,
            name: "Document list scope control",
            token_hash: &token_hash(&key.secret),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: Some(desk.world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    key
}

async fn list(desk: &Desk, token: &str, query: &NodeDocumentListQuery) -> RawReply {
    desk.world
        .app
        .call(NodeDocumentList, &[&desk.topic])
        .token(token)
        .query(query)
        .send_bytes()
        .await
}

fn shown(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn document_list_scope_filters_before_its_native_activity_window_and_cursor() {
    let desk = Desk::new().await;
    let old = desk.make(&desk.ada, "Permitted older words").await.base.id;
    desk.world.advance(chrono::Duration::minutes(1));
    desk.make(&desk.ada, "Excluded middle words").await;
    desk.world.advance(chrono::Duration::minutes(1));
    let recent = desk.make(&desk.ada, "Permitted recent words").await.base.id;
    desk.world.advance(chrono::Duration::minutes(1));
    desk.make(&desk.ada, "Excluded newest words").await;
    let key = key(
        &desk,
        desk.ada.id(),
        vec![
            rule("NodeDocumentList.read", "document", old, "read"),
            rule("NodeDocumentList.read", "document", recent, "read"),
        ],
    )
    .await;
    let first = shown(
        list(
            &desk,
            &key.secret,
            &NodeDocumentListQuery {
                limit: Some(1),
                ..Default::default()
            },
        )
        .await,
    );
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["items"][0]["id"], json!(recent));
    assert!(!first.to_string().contains("Excluded"));
    let cursor = first["next_cursor"].as_str().unwrap().to_owned();
    assert!(cursor.ends_with(&recent.to_string()));
    let second = shown(
        list(
            &desk,
            &key.secret,
            &NodeDocumentListQuery {
                limit: Some(1),
                cursor: Some(cursor),
                ..Default::default()
            },
        )
        .await,
    );
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert_eq!(second["items"][0]["id"], json!(old));
    assert!(second["next_cursor"].is_null());
    assert!(!second.to_string().contains("Excluded"));
}

#[tokio::test]
async fn document_list_keeps_content_while_live_names_need_its_exact_operation_read() {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Intrinsic document title")
        .await
        .base
        .id;
    desk.paragraphs(&desk.ada, document, &["Intrinsic description line"])
        .await;
    let ordinary = shown(list(&desk, &desk.ada.token, &Default::default()).await);
    assert_eq!(ordinary["items"][0]["node"]["name"], "Plans");
    for external in [
        None,
        Some("DocumentGet.read"),
        Some("NodeDocumentList.read"),
    ] {
        let mut rules = vec![rule("NodeDocumentList.read", "document", document, "read")];
        if let Some(operation) = external {
            rules.push(rule(operation, "topic", desk.topic, "read"));
        }
        let key = key(&desk, desk.ada.id(), rules).await;
        let page = shown(list(&desk, &key.secret, &Default::default()).await);
        let row = &page["items"][0];
        assert_eq!(row["id"], json!(document));
        assert_eq!(row["title"], "Intrinsic document title");
        assert_eq!(row["description_line"], "Intrinsic description line");
        if external == Some("NodeDocumentList.read") {
            assert_eq!(row["node"], ordinary["items"][0]["node"]);
        } else {
            assert!(row["node"]["name"].is_null());
        }
    }
    let wrong_variant = key(
        &desk,
        desk.ada.id(),
        vec![rule(
            "NodeDocumentList.deleted",
            "document",
            document,
            "read",
        )],
    )
    .await;
    assert_eq!(
        list(&desk, &wrong_variant.secret, &Default::default())
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn document_list_private_phase_keeps_native_minimal_rows_and_filters_before_name_paging() {
    let desk = Desk::new().await;
    let maker = desk.member("maker", "developer").await;
    let manager = desk.member("listkeeper", "viewer").await;
    let mut ids = Vec::new();
    for name in [
        "Alpha excluded private",
        "Middle permitted private",
        "Zulu excluded private",
    ] {
        let mut request = body(name);
        request.access = Some(NewDocumentAccess::Private);
        let id = desk.make_body(&maker, request).await.ok().base.id;
        let page = desk
            .world
            .app
            .call(PermissionPageGet, &[&"documents", &id])
            .token(&maker.token)
            .send()
            .await
            .ok();
        desk.world
            .app
            .call(PermissionPageUpdate, &[&"documents", &id])
            .token(&maker.token)
            .raw_body(&json!({"expected_revision":page.revision,"rows":[{
                "permission":"access.manage","add":[{"holder":{"kind":"user","id":manager.id()}}]
            }]}))
            .send()
            .await
            .full_permission_page()
            .ok();
        ids.push(id);
    }
    let selected_key = key(
        &desk,
        manager.id(),
        vec![
            rule("NodeDocumentList.read", "document", ids[1], "read"),
            rule("DocumentGet.read", "document", ids[1], "read"),
        ],
    )
    .await;
    let page = shown(
        list(
            &desk,
            &selected_key.secret,
            &NodeDocumentListQuery {
                limit: Some(1),
                ..Default::default()
            },
        )
        .await,
    );
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let row = &page["items"][0];
    assert_eq!(row["id"], json!(ids[1]));
    assert_eq!(row["name"], "Middle permitted private");
    assert_eq!(row["content_hidden"], true);
    assert_eq!(row["private"], true);
    assert!(row.get("description").is_none());
    assert!(row.get("updated_at").is_none());
    assert!(page["next_cursor"].is_null());
    assert!(!page.to_string().contains("excluded private"));
    let full = desk
        .world
        .app
        .call(DocumentGet, &[&ids[1]])
        .token(&selected_key.secret)
        .send_bytes()
        .await;
    assert_eq!(full.status, StatusCode::FORBIDDEN);
    let viewer = desk.member("unassignedviewer", "viewer").await;
    let viewer_key = key(
        &desk,
        viewer.id(),
        vec![rule("NodeDocumentList.read", "document", ids[1], "read")],
    )
    .await;
    assert!(
        shown(list(&desk, &viewer_key.secret, &Default::default()).await)["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn deleted_document_list_uses_manage_and_current_ordinary_restoration_authority() {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Retained native list title")
        .await
        .base
        .id;
    desk.world
        .app
        .call(DocumentDelete, &[&document])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let query = NodeDocumentListQuery {
        deleted: Some(DeletedFilter::Only),
        ..Default::default()
    };
    let managed_key = key(
        &desk,
        desk.ada.id(),
        vec![rule(
            "NodeDocumentList.deleted",
            "document",
            document,
            "manage",
        )],
    )
    .await;
    let page = shown(list(&desk, &managed_key.secret, &query).await);
    assert_eq!(page["items"][0]["id"], json!(document));
    assert_eq!(page["items"][0]["deleted"], true);
    assert!(page["items"][0]["node"]["name"].is_null());
    let read_only = key(
        &desk,
        desk.ada.id(),
        vec![rule(
            "NodeDocumentList.deleted",
            "document",
            document,
            "read",
        )],
    )
    .await;
    assert_eq!(
        list(&desk, &read_only.secret, &query).await.status,
        StatusCode::FORBIDDEN
    );
    let viewer = desk.member("restoreviewer", "viewer").await;
    let viewer_key = key(
        &desk,
        viewer.id(),
        vec![
            rule("NodeDocumentList.deleted", "document", document, "manage"),
            rule("NodeDocumentList.deleted", "document", document, "read"),
        ],
    )
    .await;
    assert!(
        shown(list(&desk, &viewer_key.secret, &query).await)["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn document_list_uses_typed_targets_and_reloads_current_stored_key_policy() {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Current scoped document title")
        .await
        .base
        .id;
    let wrong_kind = key(
        &desk,
        desk.ada.id(),
        vec![rule(
            "NodeDocumentList.read",
            "document_container",
            document,
            "read",
        )],
    )
    .await;
    assert!(
        shown(list(&desk, &wrong_kind.secret, &Default::default()).await)["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let key = key(
        &desk,
        desk.ada.id(),
        vec![rule("NodeDocumentList.read", "document", document, "read")],
    )
    .await;
    assert_eq!(
        shown(list(&desk, &key.secret, &Default::default()).await)["items"][0]["id"],
        json!(document)
    );
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(key.id)
        .bind(serde_json::to_value(KeyScope::default()).unwrap())
        .execute(&desk.world.db.pool)
        .await
        .unwrap();
    let denied = list(&desk, &key.secret, &Default::default()).await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&denied.raw).contains("Current scoped document title"));
}

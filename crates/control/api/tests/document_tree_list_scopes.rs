//! Mixed native directory windows keep their own sources, cursor and current authority.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod desk;
mod places;
mod support;
use support::NoContentReply;

use desk::Desk;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::document_tree::*;
use orch_api_contract::documents::{DeletedFilter, DocumentDelete, NewDocumentAccess};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use serde_json::json;

fn rule(operation: &str, kind: &str, id: Id, action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},
        "allow":[{"operation":operation,"actions":[action]}]})
}

fn root(desk: &Desk, operation: &str, action: &str) -> Json {
    let mut row = rule(operation, "topic", desk.topic, action);
    row["resources"] = json!(["document_container"]);
    row
}

async fn key(desk: &Desk, user: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Native document directory scopes",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: Some(desk.world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}

async fn folder(desk: &Desk, parent: Option<Id>, name: &str) -> Id {
    let receipt = {
        let request = desk
            .world
            .app
            .call(DocumentContainerCreate, &[&desk.topic])
            .token(&desk.ada.token)
            .body(&DocumentContainerCreateBody {
                parent_container_id: parent,
                name: name.into(),
                client_id: new_id().to_string(),
            });
        request.send().await
    }
    .ok()
    .into_full()
    .unwrap();
    match receipt.entries[0].item {
        DocumentTreeItem::Container(id) => id,
        _ => panic!("the native creator must produce a folder"),
    }
}

async fn list(desk: &Desk, token: &str, query: &DocumentTreeListQuery) -> RawReply {
    let request = desk
        .world
        .app
        .call(DocumentTreeList, &[&desk.topic])
        .token(token)
        .query(query);
    request.send_bytes().await
}

fn shown(reply: RawReply) -> DocumentTreePage {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

fn items(page: &DocumentTreePage) -> Vec<DocumentTreeItem> {
    page.items.iter().map(|row| row.entry.item).collect()
}

#[tokio::test]
async fn tree_list_filters_mixed_native_rows_before_the_window_and_keeps_signed_continuation() {
    let desk = Desk::new().await;
    folder(&desk, None, "A excluded folder").await;
    let selected_folder = folder(&desk, None, "B selected folder").await;
    desk.make(&desk.ada, "C excluded document").await;
    let document = desk.make(&desk.ada, "D selected document").await.base.id;
    desk.make(&desk.ada, "Z excluded document").await;
    let rules = vec![
        root(&desk, "DocumentTreeList.view", "read"),
        rule(
            "DocumentTreeList.view",
            "document_container",
            selected_folder,
            "read",
        ),
        rule("DocumentTreeList.view", "document", document, "read"),
    ];
    let token = key(&desk, desk.ada.id(), rules.clone()).await;
    let mut query = DocumentTreeListQuery {
        limit: Some(1),
        ..Default::default()
    };
    let first = shown(list(&desk, &token, &query).await);
    assert_eq!(
        items(&first),
        vec![DocumentTreeItem::Container(selected_folder)]
    );
    assert!(!serde_json::to_string(&first).unwrap().contains("excluded"));
    let cursor = first.next_cursor.unwrap();
    let other_credential = key(&desk, desk.ada.id(), rules).await;
    query.cursor = Some(cursor.clone());
    assert_eq!(
        list(&desk, &other_credential, &query).await.status,
        StatusCode::CONFLICT
    );
    let second = shown(list(&desk, &token, &query).await);
    assert_eq!(items(&second), vec![DocumentTreeItem::Document(document)]);
    assert!(!serde_json::to_string(&second).unwrap().contains("excluded"));
    assert!(second.items[0].document.is_some());
    if let Some(cursor) = second.next_cursor {
        query.cursor = Some(cursor);
        let end = shown(list(&desk, &token, &query).await);
        assert!(end.items.is_empty());
        assert!(end.next_cursor.is_none());
    }
    query.cursor = Some(cursor);
    query.parent_container_id = Some(selected_folder);
    assert_eq!(
        list(&desk, &token, &query).await.status,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn tree_list_uses_the_actual_directory_and_its_own_name_sources_without_operation_borrowing()
{
    let desk = Desk::new().await;
    let outer = folder(&desk, None, "Protected outer directory").await;
    let inner = folder(&desk, Some(outer), "Selected inner directory").await;
    let document = {
        let request = desk
            .world
            .app
            .call(DocumentContainerDocumentCreate, &[&inner])
            .token(&desk.ada.token)
            .body(&desk::body("Intrinsic selected text"));
        request.send().await
    }
    .ok()
    .into_full()
    .unwrap()
    .base
    .id;
    let query = DocumentTreeListQuery {
        parent_container_id: Some(inner),
        ..Default::default()
    };
    for external in [
        None,
        Some("DocumentTreeEntryGet.read"),
        Some("DocumentTreeList.view"),
    ] {
        let mut rules = vec![
            rule("DocumentTreeList.view", "document_container", inner, "read"),
            rule("DocumentTreeList.view", "document", document, "read"),
        ];
        if let Some(operation) = external {
            rules.extend([
                rule(operation, "document_container", outer, "read"),
                rule(operation, "topic", desk.topic, "read"),
            ]);
        }
        let token = key(&desk, desk.ada.id(), rules).await;
        let page = shown(list(&desk, &token, &query).await);
        assert_eq!(items(&page), vec![DocumentTreeItem::Document(document)]);
        assert_eq!(
            page.directory.folder.as_ref().unwrap().entry.title,
            "Selected inner directory"
        );
        assert_eq!(page.items[0].entry.title, "Intrinsic selected text");
        if external == Some("DocumentTreeList.view") {
            assert!(
                serde_json::to_string(&page)
                    .unwrap()
                    .contains("Protected outer directory")
            );
            assert_eq!(page.directory.node.name.as_deref(), Some("Plans"));
            assert_eq!(
                page.items[0]
                    .document
                    .as_ref()
                    .unwrap()
                    .node
                    .name
                    .as_deref(),
                Some("Plans")
            );
        } else {
            assert!(
                !serde_json::to_string(&page)
                    .unwrap()
                    .contains("Protected outer directory")
            );
            assert!(page.directory.node.name.is_none());
            assert!(page.items[0].document.as_ref().unwrap().node.name.is_none());
        }
    }
    for directory in [
        rule("DocumentTreeList.view", "document", inner, "read"),
        root(&desk, "DocumentTreeList.view", "read"),
        rule(
            "DocumentTreeEntryGet.read",
            "document_container",
            inner,
            "read",
        ),
    ] {
        let token = key(
            &desk,
            desk.ada.id(),
            vec![
                directory,
                rule("DocumentTreeList.view", "document", document, "read"),
            ],
        )
        .await;
        assert_eq!(
            list(&desk, &token, &query).await.status,
            StatusCode::FORBIDDEN
        );
    }
}

#[tokio::test]
async fn tree_list_keeps_real_empty_proof_and_native_visibility_gates() {
    let desk = Desk::new().await;
    let token = key(
        &desk,
        desk.ada.id(),
        vec![root(&desk, "DocumentTreeList.view", "read")],
    )
    .await;
    assert!(
        shown(list(&desk, &token, &Default::default()).await)
            .items
            .is_empty()
    );
    let mut body = desk::body("Native private source");
    body.access = Some(NewDocumentAccess::Private);
    let document = desk.make_body(&desk.ada, body).await.ok().base.id;
    let viewer = desk.member("directory_viewer", "viewer").await;
    let token = key(
        &desk,
        viewer.id(),
        vec![
            root(&desk, "DocumentTreeList.view", "read"),
            rule("DocumentTreeList.view", "document", document, "read"),
        ],
    )
    .await;
    let page = shown(list(&desk, &token, &Default::default()).await);
    assert!(page.items.is_empty());
    assert!(
        !serde_json::to_string(&page)
            .unwrap()
            .contains("Native private source")
    );
    let stranger = desk.stranger("directory_outsider").await;
    let token = key(
        &desk,
        stranger.id(),
        vec![root(&desk, "DocumentTreeList.view", "read")],
    )
    .await;
    assert_eq!(
        list(&desk, &token, &Default::default()).await.status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn tree_list_retained_management_keeps_native_inspection_without_lending_live_names() {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Retained directory text")
        .await
        .base
        .id;
    let viewer = desk.member("retained_directory_viewer", "viewer").await;
    {
        let request = desk
            .world
            .app
            .call(DocumentDelete, &[&document])
            .token(&desk.ada.token);
        request.send().await
    }
    .no_content();
    let query = DocumentTreeListQuery {
        deleted: Some(DeletedFilter::Only),
        ..Default::default()
    };
    let manager = key(
        &desk,
        desk.ada.id(),
        vec![
            root(&desk, "DocumentTreeList.retained", "manage"),
            rule("DocumentTreeList.retained", "document", document, "manage"),
        ],
    )
    .await;
    let before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='deleted.inspected'",
    )
    .bind(document)
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap();
    let page = shown(list(&desk, &manager, &query).await);
    assert_eq!(items(&page), vec![DocumentTreeItem::Document(document)]);
    assert_eq!(page.items[0].entry.title, "Retained directory text");
    assert!(page.items[0].inspection.is_some());
    assert!(page.items[0].document.is_none());
    assert!(page.directory.node.name.is_none());
    let after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='deleted.inspected'",
    )
    .bind(document)
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        after,
        before + 1,
        "one native inspection is recorded, outside factual preparation"
    );
    let wrong_variant = key(
        &desk,
        desk.ada.id(),
        vec![
            root(&desk, "DocumentTreeList.view", "read"),
            rule("DocumentTreeList.view", "document", document, "read"),
        ],
    )
    .await;
    assert_eq!(
        list(&desk, &wrong_variant, &query).await.status,
        StatusCode::FORBIDDEN
    );
    let ordinary_viewer = key(
        &desk,
        viewer.id(),
        vec![
            root(&desk, "DocumentTreeList.retained", "manage"),
            rule("DocumentTreeList.retained", "document", document, "manage"),
        ],
    )
    .await;
    assert!(
        shown(list(&desk, &ordinary_viewer, &query).await)
            .items
            .is_empty()
    );
    let deleted: bool =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM documents WHERE id=$1")
            .bind(document)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
    assert!(deleted, "inspection never restores material");
}

#[tokio::test]
async fn tree_list_publication_rechecks_frozen_membership_after_the_actual_activity_wait() {
    for change in ["unchanged", "expired", "detached"] {
        let desk = Desk::new().await;
        let parent = folder(&desk, None, "Later valid actual directory").await;
        let document = desk
            .make(&desk.ada, "Held original directory text")
            .await
            .base
            .id;
        let token = key(
            &desk,
            desk.ada.id(),
            vec![
                root(&desk, "DocumentTreeList.view", "read"),
                rule("DocumentTreeList.view", "document", document, "read"),
                rule(
                    "DocumentTreeList.view",
                    "document_container",
                    parent,
                    "read",
                ),
            ],
        )
        .await;
        let control = shown(list(&desk, &token, &Default::default()).await);
        assert!(items(&control).contains(&DocumentTreeItem::Document(document)));
        let mut blocker = desk.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = desk.world.app.clone();
        let topic = desk.topic;
        let request_token = token.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(DocumentTreeList, &[&topic])
                .token(&request_token)
                .query(&DocumentTreeListQuery::default());
            request.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait, async {
            loop {
                assert!(!request.is_finished(), "the native page must reach actual activity bookkeeping");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&desk.world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual directory result must wait at its activity append");
        if change == "expired" {
            desk.world.advance(chrono::Duration::hours(2));
        }
        if change == "detached" {
            sqlx::query("UPDATE documents SET parent_container_id=$2 WHERE id=$1")
                .bind(document)
                .bind(parent)
                .execute(&desk.world.db.pool)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        if change == "unchanged" {
            assert_eq!(items(&shown(reply)), items(&control));
        } else {
            assert_eq!(
                reply.status,
                if change == "expired" {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::FORBIDDEN
                }
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains("Held original directory text"));
        }
        if change == "detached" {
            let current = shown(
                list(
                    &desk,
                    &token,
                    &DocumentTreeListQuery {
                        parent_container_id: Some(parent),
                        ..Default::default()
                    },
                )
                .await,
            );
            assert_eq!(items(&current), vec![DocumentTreeItem::Document(document)]);
        }
    }
}

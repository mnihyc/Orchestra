//! Retained restoration keeps real source/directory authority and completed Read independent.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod desk;
mod places;
mod support;
use desk::Desk;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::{ErrorCode, Problem};
use orch_api_contract::document_tree::{
    DocumentContainerCreate, DocumentContainerCreateBody, DocumentContainerDocumentCreate,
    DocumentTreeCommand, DocumentTreeDeletePreview, DocumentTreeItem, DocumentTreeOperate,
    DocumentTreeOperationBody,
};
use orch_api_contract::documents::{DocumentDelete, DocumentRestore, DocumentRestoreBody};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::NoContentReply;
fn rule(operation: &str, kind: &str, id: Id, reach: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":actions}]})
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
            name: "Native retained restoration",
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
fn rules(document: Id, parent: Id, parent_kind: &str, read: bool) -> Vec<Json> {
    vec![
        rule(
            "DocumentRestore",
            "document",
            document,
            "self",
            if read {
                &["manage", "read"]
            } else {
                &["manage"]
            },
        ),
        rule("DocumentRestore", parent_kind, parent, "self", &["write"]),
    ]
}
async fn restore(desk: &Desk, token: &str, document: Id, expected: Option<Id>) -> RawReply {
    let request = desk
        .world
        .app
        .call(DocumentRestore, &[&document])
        .token(token)
        .body(&DocumentRestoreBody {
            expected_deletion_id: expected,
        });
    request.send_bytes().await
}
fn accepted(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn deleted(desk: &Desk, document: Id) -> Id {
    {
        let request = desk
            .world
            .app
            .call(DocumentDelete, &[&document])
            .token(&desk.ada.token);
        request.send().await
    }
    .no_content();
    orch_db::queries::trace::life(&mut *desk.world.conn().await, "document", document)
        .await
        .unwrap()
        .unwrap()
        .deletion_id
        .unwrap()
}
async fn saved(desk: &Desk, document: Id) -> (bool, Id, String, i64) {
    sqlx::query_as(
        "SELECT deleted_at IS NOT NULL,node_id,xmin::text,doc_seq FROM documents WHERE id=$1",
    )
    .bind(document)
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap()
}
async fn audits(desk: &Desk, document: Id) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='document.restored'",
    )
    .bind(document)
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap()
}
async fn folder(desk: &Desk, name: &str) -> Id {
    let value = {
        let request = desk
            .world
            .app
            .call(DocumentContainerCreate, &[&desk.topic])
            .token(&desk.ada.token)
            .body(&DocumentContainerCreateBody {
                parent_container_id: None,
                name: name.into(),
                client_id: new_id().to_string(),
            });
        request.send().await
    }
    .ok()
    .into_full()
    .unwrap();
    match value.entries[0].item {
        DocumentTreeItem::Container(id) => id,
        _ => panic!("native folder expected"),
    }
}
#[tokio::test]
async fn document_restore_manage_and_actual_directory_write_keep_the_effect_without_borrowing_get_read()
 {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Retained write-only source")
        .await
        .base
        .id;
    let deletion = deleted(&desk, document).await;
    let before = saved(&desk, document).await;
    let mut grants = rules(document, desk.topic, "topic", false);
    grants.push(rule(
        "DocumentGet.read",
        "document",
        document,
        "self",
        &["read"],
    ));
    let token = key(&desk, desk.ada.id(), grants).await;
    assert_eq!(
        accepted(restore(&desk, &token, document, Some(deletion)).await),
        json!({"projection":"receipt","document_id":document})
    );
    let after = saved(&desk, document).await;
    assert!(!after.0);
    assert!(after.3 > before.3);
    assert_eq!(audits(&desk, document).await, 1);
    let other = desk
        .make(&desk.ada, "Missing recorded directory scope")
        .await
        .base
        .id;
    deleted(&desk, other).await;
    let missing = key(
        &desk,
        desk.ada.id(),
        vec![rule(
            "DocumentRestore",
            "document",
            other,
            "self",
            &["manage", "read"],
        )],
    )
    .await;
    assert_eq!(
        restore(&desk, &missing, other, None).await.status,
        StatusCode::FORBIDDEN
    );
    assert!(saved(&desk, other).await.0);
}
#[tokio::test]
async fn document_restore_own_read_returns_the_native_live_document_and_preserves_retained_authority()
 {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Restored native content")
        .await
        .base
        .id;
    let deletion = deleted(&desk, document).await;
    let token = key(
        &desk,
        desk.ada.id(),
        rules(document, desk.topic, "topic", true),
    )
    .await;
    let value = accepted(restore(&desk, &token, document, Some(deletion)).await);
    assert!(value.get("projection").is_none());
    assert_eq!(value["id"], json!(document));
    assert_eq!(value["title"], "Restored native content");
    assert_eq!(value["deleted"], false);
    assert_eq!(
        restore(&desk, &token, document, None).await.status,
        StatusCode::NOT_FOUND
    );
    let other = desk
        .make(&desk.ada, "Viewer has no retained management")
        .await
        .base
        .id;
    deleted(&desk, other).await;
    let viewer = desk.member("restore-viewer", "viewer").await;
    let denied = key(&desk, viewer.id(), rules(other, desk.topic, "topic", true)).await;
    assert_eq!(
        restore(&desk, &denied, other, None).await.status,
        StatusCode::FORBIDDEN
    );
    assert!(saved(&desk, other).await.0);
}
#[tokio::test]
async fn document_restore_uses_the_actual_stored_folder_instead_of_its_node_or_an_id_alias() {
    let desk = Desk::new().await;
    let parent = folder(&desk, "Actual retained parent").await;
    let document = {
        let request = desk
            .world
            .app
            .call(DocumentContainerDocumentCreate, &[&parent])
            .token(&desk.ada.token)
            .body(&desk::body("Folder-contained retained text"));
        request.send().await
    }
    .ok()
    .into_full()
    .unwrap()
    .base
    .id;
    deleted(&desk, document).await;
    let node = key(
        &desk,
        desk.ada.id(),
        rules(document, desk.topic, "topic", false),
    )
    .await;
    assert_eq!(
        restore(&desk, &node, document, None).await.status,
        StatusCode::FORBIDDEN
    );
    let wrong = key(
        &desk,
        desk.ada.id(),
        rules(document, parent, "document", false),
    )
    .await;
    assert_eq!(
        restore(&desk, &wrong, document, None).await.status,
        StatusCode::FORBIDDEN
    );
    let actual = key(
        &desk,
        desk.ada.id(),
        rules(document, parent, "document_container", false),
    )
    .await;
    assert_eq!(
        accepted(restore(&desk, &actual, document, None).await),
        json!({"projection":"receipt","document_id":document})
    );
    let current: Option<Id> =
        sqlx::query_scalar("SELECT parent_container_id FROM documents WHERE id=$1")
            .bind(document)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
    assert_eq!(current, Some(parent));
    assert_eq!(audits(&desk, document).await, 1);
}
#[tokio::test]
async fn document_restore_keeps_expected_deletion_and_parent_first_without_disclosing_unread_metadata()
 {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Selected retained generation")
        .await
        .base
        .id;
    deleted(&desk, document).await;
    let token = key(
        &desk,
        desk.ada.id(),
        rules(document, desk.topic, "topic", false),
    )
    .await;
    let refused = restore(&desk, &token, document, Some(new_id())).await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    let problem: Problem = serde_json::from_slice(&refused.raw).unwrap();
    assert_eq!(problem.code, ErrorCode::ConflictStaleRevision);
    assert!(problem.extra.get("current_deletion_id").is_none());
    assert!(saved(&desk, document).await.0);
    assert_eq!(audits(&desk, document).await, 0);
    let parent = folder(&desk, "Retained ancestor requiring restoration").await;
    let nested = {
        let request = desk
            .world
            .app
            .call(DocumentContainerDocumentCreate, &[&parent])
            .token(&desk.ada.token)
            .body(&desk::body("Nested retained generation"));
        request.send().await
    }
    .ok()
    .into_full()
    .unwrap()
    .base
    .id;
    let mut body = DocumentTreeOperationBody {
        command: DocumentTreeCommand::Delete,
        items: vec![DocumentTreeItem::Container(parent)],
        to: None,
        name: None,
        client_id: new_id().to_string(),
        version: None,
        deletion_plan: None,
    };
    body.deletion_plan = Some(
        ({
            let request = desk
                .world
                .app
                .call(DocumentTreeDeletePreview, &[])
                .token(&desk.ada.token)
                .body(&body);
            request.send().await
        }
        .ok()
        .into_full()
        .expect("the ordinary deletion fixture receives its complete preview")
        .stamp)
            .into(),
    );
    {
        let request = desk
            .world
            .app
            .call(DocumentTreeOperate, &[])
            .token(&desk.ada.token)
            .body(&body);
        request.send().await
    }
    .ok();
    let token = key(
        &desk,
        desk.ada.id(),
        rules(nested, parent, "document_container", false),
    )
    .await;
    let refused = restore(&desk, &token, nested, None).await;
    let problem: Problem = serde_json::from_slice(&refused.raw).unwrap();
    assert_eq!(problem.code, ErrorCode::NodeRestoreParentFirst);
    assert!(problem.extra.get("parent").is_none());
    assert!(saved(&desk, nested).await.0);
    assert_eq!(audits(&desk, nested).await, 0);
}
#[tokio::test]
async fn document_restore_rechecks_expiry_after_its_actual_retained_row_wait_before_update() {
    for expire in [false, true] {
        let desk = Desk::new().await;
        let document = desk
            .make(&desk.ada, "Waiting retained source")
            .await
            .base
            .id;
        deleted(&desk, document).await;
        let before = saved(&desk, document).await;
        let token = key(
            &desk,
            desk.ada.id(),
            rules(document, desk.topic, "topic", false),
        )
        .await;
        let mut blocker = desk.world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM documents WHERE id=$1 FOR UPDATE")
            .bind(document)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = desk.world.app.clone();
        let request = tokio::spawn(async move {
            let request =
                app.call(DocumentRestore, &[&document])
                    .token(&token)
                    .body(&DocumentRestoreBody {
                        expected_deletion_id: None,
                    });
            request.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async {loop {
            assert!(!request.is_finished(),"the actual retained document row wait was not reached");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM documents%' AND query LIKE '%FOR UPDATE%')")
                .bind(pid).fetch_one(&desk.world.db.pool).await.unwrap();if blocked {break;}tokio::task::yield_now().await;
        }}).await.expect("the actual native retained row must wait");
        if expire {
            desk.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert_eq!(saved(&desk, document).await, before);
            assert_eq!(audits(&desk, document).await, 0);
        } else {
            assert_eq!(
                accepted(reply),
                json!({"projection":"receipt","document_id":document})
            );
            assert!(!saved(&desk, document).await.0);
            assert_eq!(audits(&desk, document).await, 1);
        }
    }
}
#[tokio::test]
async fn document_restore_publication_keeps_the_committed_restoration_without_replaying() {
    for change in ["unchanged", "expired", "read_lost"] {
        let desk = Desk::new().await;
        let document = desk
            .make(&desk.ada, "Held committed restoration")
            .await
            .base
            .id;
        deleted(&desk, document).await;
        let replacement = desk.another_topic("Later restored location").await;
        let grants = vec![
            rule("DocumentRestore", "document", document, "self", &["manage"]),
            rule(
                "DocumentRestore",
                "topic",
                desk.topic,
                "subtree",
                &["write", "read"],
            ),
        ];
        let token = key(&desk, desk.ada.id(), grants).await;
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
        let request = tokio::spawn(async move {
            let request =
                app.call(DocumentRestore, &[&document])
                    .token(&token)
                    .body(&DocumentRestoreBody {
                        expected_deletion_id: None,
                    });
            request.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async {loop {
            assert!(!request.is_finished(),"the committed restoration did not reach final activity bookkeeping");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&desk.world.db.pool).await.unwrap();if blocked {break;}tokio::task::yield_now().await;
        }}).await.expect("the actual committed restoration result must wait at activity append");
        assert!(!saved(&desk, document).await.0);
        assert_eq!(audits(&desk, document).await, 1);
        if change == "expired" {
            desk.world.advance(chrono::Duration::hours(2));
        }
        if change == "read_lost" {
            sqlx::query("UPDATE documents SET node_id=$2 WHERE id=$1")
                .bind(document)
                .bind(replacement)
                .execute(&desk.world.db.pool)
                .await
                .unwrap();
        }
        let committed = saved(&desk, document).await;
        blocker.commit().await.unwrap();
        let value = accepted(tokio::time::timeout(wait, request).await.unwrap().unwrap());
        if change == "unchanged" {
            assert!(value.get("projection").is_none());
            assert_eq!(value["id"], json!(document));
        } else {
            assert_eq!(
                value,
                json!({"projection":"receipt","document_id":document})
            );
        }
        assert_eq!(saved(&desk, document).await, committed);
        assert_eq!(audits(&desk, document).await, 1);
    }
}

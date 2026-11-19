//! Native folder creation, actual destinations, durable replay and completed source publication.
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
use orch_api_contract::document_tree::{DocumentContainerCreate, DocumentContainerCreateBody};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::{
    documents::containers,
    identity::{self, NewApiToken},
};
use orch_types::{Id, Json, new_id};
use serde_json::json;

fn rule(operation: &str, kind: &str, id: Id, reach: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":actions}]})
}
async fn key(desk: &Desk, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: desk.ada.id(),
            name: "Native folder creation scope",
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
fn body(parent: Option<Id>, name: &str, client: &str) -> DocumentContainerCreateBody {
    DocumentContainerCreateBody {
        parent_container_id: parent,
        name: name.into(),
        client_id: client.into(),
    }
}
async fn create(
    desk: &Desk,
    token: &str,
    node: Id,
    body: &DocumentContainerCreateBody,
) -> RawReply {
    desk.world
        .app
        .call(DocumentContainerCreate, &[&node])
        .token(token)
        .body(body)
        .send_bytes()
        .await
}
fn accepted(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn saved(desk: &Desk, name: &str) -> Option<(Id, String)> {
    sqlx::query_as("SELECT id,xmin::text FROM document_containers WHERE node_id=$1 AND name=$2")
        .bind(desk.topic)
        .bind(name)
        .fetch_optional(&desk.world.db.pool)
        .await
        .unwrap()
}
async fn ordinary(desk: &Desk, parent: Option<Id>, name: &str) -> Id {
    let receipt = desk
        .world
        .app
        .call(DocumentContainerCreate, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&body(parent, name, &new_id().to_string()))
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    match receipt.entries[0].item {
        orch_api_contract::document_tree::DocumentTreeItem::Container(id) => id,
        _ => panic!("the native folder creator returned another kind"),
    }
}

#[tokio::test]
async fn folder_creation_write_only_keeps_its_real_identity_and_replay_without_borrowing_get_read()
{
    let desk = Desk::new().await;
    let token = key(
        &desk,
        vec![
            rule(
                "DocumentContainerCreate",
                "topic",
                desk.topic,
                "self",
                &["write"],
            ),
            rule(
                "DocumentContainerGet.read",
                "topic",
                desk.topic,
                "subtree",
                &["read"],
            ),
        ],
    )
    .await;
    let input = body(None, "Write-only research", "stable-private-creation");
    let first = accepted(create(&desk, &token, desk.topic, &input).await);
    let stored = saved(&desk, "Write-only research").await.unwrap();
    let operation: Id = serde_json::from_value(first["operation"].clone()).unwrap();
    assert_eq!(
        first,
        json!({"projection":"receipt","operation":operation,"container_id":stored.0})
    );
    let repeated = accepted(create(&desk, &token, desk.topic, &input).await);
    assert_eq!(repeated, first);
    assert_eq!(saved(&desk, "Write-only research").await.unwrap(), stored);
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM document_tree_operations WHERE actor_kind='person' AND actor_id=$1 AND request_ref=$2")
        .bind(desk.ada.id()).bind(&input.client_id).fetch_one(&desk.world.db.pool).await.unwrap();
    assert_eq!(count, 1);
    let conflict = body(
        Some(stored.0),
        "Different request",
        "stable-private-creation",
    );
    assert_eq!(
        create(&desk, &token, desk.topic, &conflict).await.status,
        StatusCode::FORBIDDEN,
        "the changed actual destination needs its own scope before native replay is considered"
    );
}

#[tokio::test]
async fn folder_creation_uses_the_stored_parent_and_scopes_current_ancestor_labels() {
    let desk = Desk::new().await;
    let parent = ordinary(&desk, None, "Protected existing parent").await;
    let node_only = key(
        &desk,
        vec![rule(
            "DocumentContainerCreate",
            "topic",
            desk.topic,
            "self",
            &["write", "read"],
        )],
    )
    .await;
    assert_eq!(
        create(
            &desk,
            &node_only,
            desk.topic,
            &body(Some(parent), "Must not exist", "wrong-parent-target")
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert!(saved(&desk, "Must not exist").await.is_none());
    let correct = key(
        &desk,
        vec![rule(
            "DocumentContainerCreate",
            "document_container",
            parent,
            "subtree",
            &["write", "read"],
        )],
    )
    .await;
    let full = accepted(
        create(
            &desk,
            &correct,
            desk.topic,
            &body(Some(parent), "Created inner folder", "actual-parent-create"),
        )
        .await,
    );
    assert!(full.get("projection").is_none());
    assert_eq!(full["entries"][0]["title"], "Created inner folder");
    assert!(full.to_string().contains("Protected existing parent"));
    assert!(!full.to_string().contains("Acme"));
    assert!(!full.to_string().contains("Plans"));
    let elsewhere = desk.another_topic("Other native area").await;
    assert_eq!(
        create(
            &desk,
            &correct,
            elsewhere,
            &body(Some(parent), "Wrong area", "not-an-area-alias")
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    let wrong_kind = key(
        &desk,
        vec![rule(
            "DocumentContainerCreate",
            "document",
            parent,
            "self",
            &["write", "read"],
        )],
    )
    .await;
    assert_eq!(
        create(
            &desk,
            &wrong_kind,
            desk.topic,
            &body(Some(parent), "Wrong native kind", "not-a-document-target")
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn folder_creation_rechecks_key_expiry_after_the_actual_native_scope_wait() {
    for expire in [false, true] {
        let desk = Desk::new().await;
        let token = key(
            &desk,
            vec![rule(
                "DocumentContainerCreate",
                "topic",
                desk.topic,
                "self",
                &["write"],
            )],
        )
        .await;
        let mut setup = desk.world.db.pool.begin().await.unwrap();
        containers::lock_scopes(&mut setup, &[desk.topic])
            .await
            .unwrap();
        setup.commit().await.unwrap();
        let mut blocker = desk.world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT node_id FROM document_tree_scopes WHERE node_id=$1 FOR UPDATE")
            .bind(desk.topic)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = desk.world.app.clone();
        let node = desk.topic;
        let request = tokio::spawn(async move {
            app.call(DocumentContainerCreate, &[&node])
                .token(&token)
                .body(&body(
                    None,
                    "Actual native scope wait",
                    "native-lock-create",
                ))
                .send_bytes()
                .await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async{
            loop {
                assert!(!request.is_finished(),"the native scope-lock wait was not reached");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%SELECT document_tree_lock(%')")
                    .bind(pid).fetch_one(&desk.world.db.pool).await.unwrap();
                if blocked{break;}
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual document-area serialization lock must wait");
        assert!(saved(&desk, "Actual native scope wait").await.is_none());
        if expire {
            desk.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert!(saved(&desk, "Actual native scope wait").await.is_none());
        } else {
            let id = saved(&desk, "Actual native scope wait").await.unwrap().0;
            assert_eq!(accepted(reply)["container_id"], json!(id));
        }
    }
}

#[tokio::test]
async fn folder_creation_keeps_committed_receipt_identity_and_xmin_after_real_publication_wait() {
    for expire in [false, true] {
        let desk = Desk::new().await;
        let token = key(
            &desk,
            vec![rule(
                "DocumentContainerCreate",
                "topic",
                desk.topic,
                "subtree",
                &["write", "read"],
            )],
        )
        .await;
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
        let node = desk.topic;
        let request = tokio::spawn(async move {
            app.call(DocumentContainerCreate, &[&node])
                .token(&token)
                .body(&body(None, "Committed held folder", "held-native-create"))
                .send_bytes()
                .await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async{
            loop {
                assert!(!request.is_finished(),"the native completed result did not reach activity bookkeeping");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&desk.world.db.pool).await.unwrap();
                if blocked{break;}
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual completed native response must wait at its activity append");
        let before = saved(&desk, "Committed held folder").await.unwrap();
        let operation:Id=sqlx::query_scalar("SELECT operation_id FROM document_tree_operations WHERE actor_kind='person' AND actor_id=$1 AND request_ref=$2")
            .bind(desk.ada.id()).bind("held-native-create").fetch_one(&desk.world.db.pool).await.unwrap();
        if expire {
            desk.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = accepted(tokio::time::timeout(wait, request).await.unwrap().unwrap());
        if expire {
            assert_eq!(
                reply,
                json!({"projection":"receipt","operation":operation,"container_id":before.0})
            );
        } else {
            assert_eq!(reply["operation"], json!(operation));
            assert_eq!(reply["entries"][0]["title"], "Committed held folder");
        }
        assert_eq!(
            saved(&desk, "Committed held folder").await.unwrap(),
            before,
            "publication cannot repeat the creator"
        );
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='document.created'",
        )
        .bind(before.0)
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap();
        assert_eq!(audits, 1);
    }
}

#[tokio::test]
async fn folder_creation_replay_keeps_the_original_generated_id_after_its_row_was_purged() {
    let desk = Desk::new().await;
    let token = key(
        &desk,
        vec![rule(
            "DocumentContainerCreate",
            "topic",
            desk.topic,
            "self",
            &["write"],
        )],
    )
    .await;
    let input = body(None, "Purgable native result", "durable-purged-result");
    let before = accepted(create(&desk, &token, desk.topic, &input).await);
    let id: Id = serde_json::from_value(before["container_id"].clone()).unwrap();
    sqlx::query("DELETE FROM document_containers WHERE id=$1")
        .bind(id)
        .execute(&desk.world.db.pool)
        .await
        .unwrap();
    assert!(saved(&desk, "Purgable native result").await.is_none());
    let replay = accepted(create(&desk, &token, desk.topic, &input).await);
    assert_eq!(replay, before);
    assert!(
        saved(&desk, "Purgable native result").await.is_none(),
        "native replay cannot recreate a purged result"
    );
}

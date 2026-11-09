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
    let request = desk
        .world
        .app
        .call(DocumentContainerCreate, &[&node])
        .token(token)
        .body(body);
    request.send_bytes().await
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
    let receipt = {
        let request = desk
            .world
            .app
            .call(DocumentContainerCreate, &[&desk.topic])
            .token(&desk.ada.token)
            .body(&body(parent, name, &new_id().to_string()));
        request.send().await
    }
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
        json!({"projection":"receipt","operation":operation,"folder_id":stored.0})
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
        StatusCode::CONFLICT
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
            let request = app
                .call(DocumentContainerCreate, &[&node])
                .token(&token)
                .body(&body(
                    None,
                    "Actual native scope wait",
                    "native-lock-create",
                ));
            request.send_bytes().await
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
            assert_eq!(accepted(reply)["folder_id"], json!(id));
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
            let request = app
                .call(DocumentContainerCreate, &[&node])
                .token(&token)
                .body(&body(None, "Committed held folder", "held-native-create"));
            request.send_bytes().await
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
                json!({"projection":"receipt","operation":operation,"folder_id":before.0})
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
    let header = new_id().to_string();
    let create_keyed = || {
        let request = desk
            .world
            .app
            .call(DocumentContainerCreate, &[&desk.topic])
            .token(&token)
            .header(orch_api_contract::common::IDEMPOTENCY_HEADER, &header)
            .body(&input);
        async move { request.send_bytes().await }
    };
    let before = accepted(create_keyed().await);
    let id: Id = serde_json::from_value(before["folder_id"].clone()).unwrap();
    sqlx::query("DELETE FROM document_containers WHERE id=$1")
        .bind(id)
        .execute(&desk.world.db.pool)
        .await
        .unwrap();
    assert!(saved(&desk, "Purgable native result").await.is_none());
    let replay = accepted(create_keyed().await);
    assert_eq!(replay, before);
    assert!(
        saved(&desk, "Purgable native result").await.is_none(),
        "native replay cannot recreate a purged result"
    );
    assert_eq!(
        create(&desk, &token, desk.topic, &input).await.status,
        StatusCode::NOT_FOUND,
        "the independent native body replay still cannot read a purged result"
    );
    let operations:i64=sqlx::query_scalar("SELECT count(*) FROM document_tree_operations WHERE actor_kind='person' AND actor_id=$1 AND request_ref=$2").bind(desk.ada.id()).bind(&input.client_id).fetch_one(&desk.world.db.pool).await.unwrap();
    assert_eq!(
        operations, 1,
        "neither replay recreates its purged folder or operation"
    );
}

async fn text_create(
    desk: &Desk,
    token: &str,
    parent: Id,
    in_folder: bool,
    input: &orch_api_contract::documents::NodeDocumentCreateBody,
) -> RawReply {
    if in_folder {
        let request = desk
            .world
            .app
            .call(
                orch_api_contract::document_tree::DocumentContainerDocumentCreate,
                &[&parent],
            )
            .token(token)
            .body(input);
        request.send_bytes().await
    } else {
        let request = desk
            .world
            .app
            .call(orch_api_contract::documents::NodeDocumentCreate, &[&parent])
            .token(token)
            .body(input);
        request.send_bytes().await
    }
}
fn text_operation(in_folder: bool) -> &'static str {
    if in_folder {
        "DocumentContainerDocumentCreate"
    } else {
        "NodeDocumentCreate"
    }
}
async fn text_saved(desk: &Desk, title: &str) -> Option<(Id, String, i64)> {
    sqlx::query_as("SELECT id,xmin::text,doc_seq FROM documents WHERE node_id=$1 AND title=$2")
        .bind(desk.topic)
        .bind(title)
        .fetch_optional(&desk.world.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn text_creation_node_write_only_keeps_private_content_without_borrowing_another_read() {
    let desk = Desk::new().await;
    let token = key(
        &desk,
        vec![
            rule(
                "NodeDocumentCreate",
                "topic",
                desk.topic,
                "self",
                &["write"],
            ),
            rule(
                "DocumentGet.read",
                "topic",
                desk.topic,
                "subtree",
                &["read"],
            ),
        ],
    )
    .await;
    let mut input = desk::body("Private scripted text");
    input.markdown = Some("My own initial content".into());
    input.access = Some(orch_api_contract::documents::NewDocumentAccess::Private);
    let reply = accepted(text_create(&desk, &token, desk.topic, false, &input).await);
    let saved = text_saved(&desk, &input.title).await.unwrap();
    assert_eq!(reply, json!({"projection":"receipt","document_id":saved.0}));
    let blocks = orch_db::queries::documents::live_blocks(&mut *desk.world.conn().await, saved.0)
        .await
        .unwrap();
    assert_eq!(blocks.len(), 1);
    assert!(
        blocks[0]
            .content
            .to_string()
            .contains("My own initial content")
    );
    let native = {
        let request = desk
            .world
            .app
            .call(orch_api_contract::documents::DocumentGet, &[&saved.0])
            .token(&desk.ada.token);
        request.send().await
    }
    .ok();
    assert_eq!(native.base.id, saved.0);
    assert_eq!(native.base.title, input.title);
    let wrong = key(
        &desk,
        vec![rule(
            "NodeDocumentCreate",
            "document_container",
            desk.topic,
            "self",
            &["write"],
        )],
    )
    .await;
    assert_eq!(
        text_create(
            &desk,
            &wrong,
            desk.topic,
            false,
            &desk::body("Wrong directory kind")
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert!(text_saved(&desk, "Wrong directory kind").await.is_none());
}

#[tokio::test]
async fn text_creation_uses_the_actual_container_and_its_own_response_read() {
    let desk = Desk::new().await;
    let parent = ordinary(&desk, None, "Existing text parent").await;
    let node_only = key(
        &desk,
        vec![rule(
            "DocumentContainerDocumentCreate",
            "topic",
            desk.topic,
            "self",
            &["write", "read"],
        )],
    )
    .await;
    assert_eq!(
        text_create(
            &desk,
            &node_only,
            parent,
            true,
            &desk::body("Outside actual folder scope")
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert!(
        text_saved(&desk, "Outside actual folder scope")
            .await
            .is_none()
    );
    let write = key(
        &desk,
        vec![rule(
            "DocumentContainerDocumentCreate",
            "document_container",
            parent,
            "self",
            &["write"],
        )],
    )
    .await;
    let safe = accepted(
        text_create(
            &desk,
            &write,
            parent,
            true,
            &desk::body("Stored folder text"),
        )
        .await,
    );
    let stored = text_saved(&desk, "Stored folder text").await.unwrap();
    assert_eq!(safe, json!({"projection":"receipt","document_id":stored.0}));
    let full_key = key(
        &desk,
        vec![rule(
            "DocumentContainerDocumentCreate",
            "document_container",
            parent,
            "subtree",
            &["write", "read"],
        )],
    )
    .await;
    let full = accepted(
        text_create(
            &desk,
            &full_key,
            parent,
            true,
            &desk::body("Readable folder text"),
        )
        .await,
    );
    assert!(full.get("projection").is_none());
    assert!(full.to_string().contains("Readable folder text"));
    assert!(full.to_string().contains("Existing text parent"));
    assert!(!full.to_string().contains("Acme"));
    assert!(!full.to_string().contains("Plans"));
    let id = text_saved(&desk, "Readable folder text").await.unwrap().0;
    let actual: Option<Id> =
        sqlx::query_scalar("SELECT parent_container_id FROM documents WHERE id=$1")
            .bind(id)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
    assert_eq!(actual, Some(parent));
    let wrong = key(
        &desk,
        vec![rule(
            "DocumentContainerDocumentCreate",
            "document",
            parent,
            "self",
            &["write", "read"],
        )],
    )
    .await;
    assert_eq!(
        text_create(
            &desk,
            &wrong,
            parent,
            true,
            &desk::body("Wrong folder kind")
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn text_creation_copied_labels_need_the_same_operations_source_read_without_removing_the_write()
 {
    let desk = Desk::new().await;
    let other = desk.another_topic("Separate source home").await;
    let source = desk
        .make_at(&desk.ada, other, desk::body("Protected referenced title"))
        .await
        .ok()
        .base
        .id;
    for in_folder in [false, true] {
        let parent = if in_folder {
            ordinary(&desk, None, "Reference destination").await
        } else {
            desk.topic
        };
        let kind = if in_folder {
            "document_container"
        } else {
            "topic"
        };
        let op = text_operation(in_folder);
        for (index, source_read, explicit) in [
            (0, false, None),
            (1, true, None),
            (2, false, Some("Client supplied label")),
        ] {
            let mut rules = vec![rule(op, kind, parent, "self", &["write"])];
            if source_read {
                rules.push(rule(op, "document", source, "self", &["read"]));
            } else {
                rules.push(rule(
                    "DocumentGet.read",
                    "document",
                    source,
                    "self",
                    &["read"],
                ));
            }
            let token = key(&desk, rules).await;
            let block = new_id();
            let mut reference = json!({"type":"ref","kind":"document","id":source});
            if let Some(label) = explicit {
                reference["label"] = json!(label);
            }
            let mut input = desk::body(&format!("Copied source {in_folder} {index}"));
            input.blocks = Some(vec![
                serde_json::from_value(json!({
                    "id":block,"type":"paragraph","content":[reference]
                }))
                .unwrap(),
            ]);
            let reply = accepted(text_create(&desk, &token, parent, in_folder, &input).await);
            let id = text_saved(&desk, &input.title).await.unwrap().0;
            assert_eq!(reply, json!({"projection":"receipt","document_id":id}));
            let content: Json = sqlx::query_scalar(
                "SELECT content FROM document_blocks WHERE id=$1 AND document_id=$2",
            )
            .bind(block)
            .bind(id)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
            assert_eq!(content[0]["id"], json!(source));
            if source_read {
                assert_eq!(content[0]["label"], "Protected referenced title");
            } else if let Some(label) = explicit {
                assert_eq!(content[0]["label"], label);
            } else {
                assert!(content[0].get("label").is_none() || content[0]["label"].is_null());
            }
        }
    }
}

#[tokio::test]
async fn text_creation_rechecks_expiry_after_the_actual_directory_lock_before_a_header() {
    for in_folder in [false, true] {
        for expire in [false, true] {
            let desk = Desk::new().await;
            let parent = if in_folder {
                ordinary(&desk, None, "Waiting text parent").await
            } else {
                desk.topic
            };
            let kind = if in_folder {
                "document_container"
            } else {
                "topic"
            };
            let token = key(
                &desk,
                vec![rule(
                    text_operation(in_folder),
                    kind,
                    parent,
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
            let request = tokio::spawn(async move {
                let body = desk::body("Actual waiting text header");
                if in_folder {
                    let request = app
                        .call(
                            orch_api_contract::document_tree::DocumentContainerDocumentCreate,
                            &[&parent],
                        )
                        .token(&token)
                        .body(&body);
                    request.send_bytes().await
                } else {
                    let request = app
                        .call(orch_api_contract::documents::NodeDocumentCreate, &[&parent])
                        .token(&token)
                        .body(&body);
                    request.send_bytes().await
                }
            });
            let wait = std::time::Duration::from_secs(30);
            tokio::time::timeout(wait,async {
                loop {
                    assert!(!request.is_finished(),"the actual text directory lock was not reached");
                    let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                        AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%SELECT document_tree_lock(%')")
                        .bind(pid).fetch_one(&desk.world.db.pool).await.unwrap();
                    if blocked {break;}
                    tokio::task::yield_now().await;
                }
            }).await.expect("the actual native text directory serialization must wait");
            assert!(
                text_saved(&desk, "Actual waiting text header")
                    .await
                    .is_none()
            );
            if expire {
                desk.world.advance(chrono::Duration::hours(2));
            }
            blocker.commit().await.unwrap();
            let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
            if expire {
                assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
                assert!(
                    text_saved(&desk, "Actual waiting text header")
                        .await
                        .is_none()
                );
            } else {
                let id = text_saved(&desk, "Actual waiting text header")
                    .await
                    .unwrap()
                    .0;
                assert_eq!(accepted(reply)["document_id"], json!(id));
            }
        }
    }
}

#[tokio::test]
async fn text_creation_publication_keeps_committed_identity_and_content_without_replaying() {
    for in_folder in [false, true] {
        for expire in [false, true] {
            let desk = Desk::new().await;
            let parent = if in_folder {
                ordinary(&desk, None, "Held text parent").await
            } else {
                desk.topic
            };
            let kind = if in_folder {
                "document_container"
            } else {
                "topic"
            };
            let token = key(
                &desk,
                vec![rule(
                    text_operation(in_folder),
                    kind,
                    parent,
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
            let request = tokio::spawn(async move {
                let mut body = desk::body("Held committed text");
                body.markdown = Some("Initial material written once".into());
                if in_folder {
                    let request = app
                        .call(
                            orch_api_contract::document_tree::DocumentContainerDocumentCreate,
                            &[&parent],
                        )
                        .token(&token)
                        .body(&body);
                    request.send_bytes().await
                } else {
                    let request = app
                        .call(orch_api_contract::documents::NodeDocumentCreate, &[&parent])
                        .token(&token)
                        .body(&body);
                    request.send_bytes().await
                }
            });
            let wait = std::time::Duration::from_secs(30);
            tokio::time::timeout(wait,async {
                loop {
                    assert!(!request.is_finished(),"the completed text did not reach activity bookkeeping");
                    let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                        AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                        .bind(pid).fetch_one(&desk.world.db.pool).await.unwrap();
                    if blocked {break;}
                    tokio::task::yield_now().await;
                }
            }).await.expect("the actual completed text response must wait at its activity append");
            let before = text_saved(&desk, "Held committed text").await.unwrap();
            let contents: Vec<(Id, Json, String)> = sqlx::query_as(
                "SELECT id,content,xmin::text FROM document_blocks WHERE document_id=$1",
            )
            .bind(before.0)
            .fetch_all(&desk.world.db.pool)
            .await
            .unwrap();
            assert_eq!(contents.len(), 1);
            assert!(
                contents[0]
                    .1
                    .to_string()
                    .contains("Initial material written once")
            );
            if expire {
                desk.world.advance(chrono::Duration::hours(2));
            }
            blocker.commit().await.unwrap();
            let reply = accepted(tokio::time::timeout(wait, request).await.unwrap().unwrap());
            if expire {
                assert_eq!(
                    reply,
                    json!({"projection":"receipt","document_id":before.0})
                );
            } else {
                assert!(reply.get("projection").is_none());
                assert!(reply.to_string().contains("Held committed text"));
            }
            assert_eq!(
                text_saved(&desk, "Held committed text").await.unwrap(),
                before
            );
            let after: Vec<(Id, Json, String)> = sqlx::query_as(
                "SELECT id,content,xmin::text FROM document_blocks WHERE document_id=$1",
            )
            .bind(before.0)
            .fetch_all(&desk.world.db.pool)
            .await
            .unwrap();
            assert_eq!(after, contents);
            let audits: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='document.created'",
            )
            .bind(before.0)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
            assert_eq!(audits, 1);
            let receipts: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM document_tree_operations WHERE operation_id=$1",
            )
            .bind(before.0)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
            assert_eq!(receipts, 1);
        }
    }
}

#[tokio::test]
async fn text_creation_publication_rechecks_the_actual_parent_association_without_replaying() {
    let desk = Desk::new().await;
    let original = ordinary(&desk, None, "Original protected ancestor").await;
    let replacement = ordinary(&desk, None, "Replacement protected ancestor").await;
    let token = key(
        &desk,
        vec![rule(
            "DocumentContainerDocumentCreate",
            "topic",
            desk.topic,
            "subtree",
            &["write", "read"],
        )],
    )
    .await;
    let control = accepted(
        text_create(
            &desk,
            &token,
            original,
            true,
            &desk::body("Readable association control"),
        )
        .await,
    );
    assert!(control.to_string().contains("Original protected ancestor"));
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
        let request = app
            .call(
                orch_api_contract::document_tree::DocumentContainerDocumentCreate,
                &[&original],
            )
            .token(&token)
            .body(&desk::body("Held changing ancestor"));
        request.send_bytes().await
    });
    let wait = std::time::Duration::from_secs(30);
    tokio::time::timeout(wait,async {
        loop {
            assert!(!request.is_finished(),"the native held result did not reach activity bookkeeping");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&desk.world.db.pool).await.unwrap();
            if blocked {break;}
            tokio::task::yield_now().await;
        }
    }).await.expect("the actual completed native result must be waiting");
    let id = text_saved(&desk, "Held changing ancestor").await.unwrap().0;
    sqlx::query("UPDATE documents SET parent_container_id=$2 WHERE id=$1")
        .bind(id)
        .bind(replacement)
        .execute(&desk.world.db.pool)
        .await
        .unwrap();
    let after_move = text_saved(&desk, "Held changing ancestor").await.unwrap();
    blocker.commit().await.unwrap();
    let held = accepted(tokio::time::timeout(wait, request).await.unwrap().unwrap());
    assert_eq!(held, json!({"projection":"receipt","document_id":id}));
    assert_eq!(
        text_saved(&desk, "Held changing ancestor").await.unwrap(),
        after_move
    );
    let current = {
        let request = desk
            .world
            .app
            .call(orch_api_contract::documents::DocumentGet, &[&id])
            .token(&desk.ada.token);
        request.send().await
    }
    .ok();
    let current = serde_json::to_value(current).unwrap();
    assert!(
        current
            .to_string()
            .contains("Replacement protected ancestor")
    );
    assert!(!current.to_string().contains("Original protected ancestor"));
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='document.created'",
    )
    .bind(id)
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);
}

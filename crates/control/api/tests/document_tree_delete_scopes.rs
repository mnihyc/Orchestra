//! Recursive previews bind actual mixed membership and preserve private exact confirmations.
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
use orch_api_contract::document_tree::{
    DocumentContainerCreate, DocumentContainerCreateBody, DocumentContainerDocumentCreate,
    DocumentTreeCommand, DocumentTreeDeletePreview, DocumentTreeDeletion, DocumentTreeItem,
    DocumentTreeOperate, DocumentTreeOperationBody,
};
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use serde_json::json;

const OP: &str = "DocumentTreeDeletePreview";
// The same fixture watchdog as the existing scope wait controls, never an authority lifetime.
const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, kind: &str, id: Id, reach: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":actions}]})
}
fn scope(root: Id, read: bool) -> Vec<Json> {
    vec![rule(
        OP,
        "document_container",
        root,
        "subtree",
        if read {
            &["manage", "read"]
        } else {
            &["manage"]
        },
    )]
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
            name: "Native deletion preview",
            token_hash: &token_hash(&key.secret),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: Some(desk.world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    key
}
async fn folder(desk: &Desk, name: &str) -> Id {
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
    let receipt = request
        .send()
        .await
        .ok()
        .into_full()
        .expect("the native folder fixture keeps its full receipt");
    match receipt.entries[0].item {
        DocumentTreeItem::Container(id) => id,
        DocumentTreeItem::Document(_) => {
            panic!("the native folder fixture must create a container")
        }
    }
}
async fn child(desk: &Desk, parent: Id, title: &str) -> Id {
    let request = desk
        .world
        .app
        .call(DocumentContainerDocumentCreate, &[&parent])
        .token(&desk.ada.token)
        .body(&desk::body(title));
    request
        .send()
        .await
        .ok()
        .into_full()
        .expect("the native document fixture keeps full content")
        .base
        .id
}
fn body(root: Id) -> DocumentTreeOperationBody {
    DocumentTreeOperationBody {
        command: DocumentTreeCommand::Delete,
        items: vec![DocumentTreeItem::Container(root)],
        to: None,
        name: None,
        client_id: new_id().to_string(),
        version: None,
        deletion_plan: None,
    }
}
async fn preview(desk: &Desk, token: &str, body: &DocumentTreeOperationBody) -> RawReply {
    let request = desk
        .world
        .app
        .call(DocumentTreeDeletePreview, &[])
        .token(token)
        .body(body);
    request.send_bytes().await
}
async fn operate(desk: &Desk, body: &DocumentTreeOperationBody) -> RawReply {
    let request = desk
        .world
        .app
        .call(DocumentTreeOperate, &[])
        .token(&desk.ada.token)
        .body(body);
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
async fn state(desk: &Desk) -> Json {
    sqlx::query_scalar(
        "SELECT jsonb_build_array(
        (SELECT jsonb_agg(to_jsonb(c) ORDER BY id) FROM document_containers c),
        (SELECT jsonb_agg(to_jsonb(d) ORDER BY id) FROM documents d),
        (SELECT count(*) FROM document_tree_operations),
        (SELECT count(*) FROM audit_log WHERE object_kind IN ('document','document_container')))",
    )
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap()
}
async fn wait_sql(
    desk: &Desk,
    pid: i32,
    request: &tokio::task::JoinHandle<RawReply>,
    pattern: &str,
) {
    tokio::time::timeout(WAIT, async {
        loop {
            assert!(!request.is_finished(), "the actual native wait must be observed");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)").bind(pid).bind(pattern).fetch_one(&desk.world.db.pool).await.unwrap();
            if blocked { break; } tokio::task::yield_now().await;
        }
    }).await.expect("observe the exact native query and literal blocker PID");
}
fn confirmation(value: &Json) -> DocumentTreeDeletion {
    assert_eq!(value["projection"], "receipt");
    serde_json::from_value(value["deletion_plan"].clone()).unwrap()
}

#[tokio::test]
async fn deletion_preview_checks_every_native_descendant_without_operation_kind_or_action_loans() {
    let desk = Desk::new().await;
    let root = folder(&desk, "Private recursive scope").await;
    child(&desk, root, "Private descendant").await;
    let body = body(root);
    let before = state(&desk).await;
    for rules in [
        vec![rule(OP, "document_container", root, "self", &["manage"])],
        vec![rule(OP, "document", root, "subtree", &["manage"])],
        vec![rule(OP, "document_container", root, "subtree", &["read"])],
        vec![rule(
            "DocumentTreeOperate.delete",
            "document_container",
            root,
            "subtree",
            &["manage"],
        )],
    ] {
        let key = key(&desk, desk.ada.id(), rules).await;
        assert_eq!(
            preview(&desk, &key.secret, &body).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(state(&desk).await, before);
    }
    let viewer = desk.member("deletionviewer", "viewer").await;
    let key = key(&desk, viewer.id(), scope(root, true)).await;
    assert!(
        !preview(&desk, &key.secret, &body).await.status.is_success(),
        "scope confers no native Manage"
    );
    assert_eq!(state(&desk).await, before);
}

#[tokio::test]
async fn manage_only_preview_keeps_a_protected_complete_confirmation_and_its_own_read_keeps_full() {
    let desk = Desk::new().await;
    let root = folder(&desk, "Unreadable root title").await;
    let first = child(&desk, root, "Unreadable first title").await;
    let body = body(root);
    let before = state(&desk).await;
    let mut rules = scope(root, false);
    rules.push(rule(
        "DocumentTreeList.view",
        "document_container",
        root,
        "subtree",
        &["read"],
    ));
    let writer = key(&desk, desk.ada.id(), rules).await;
    let receipt = accepted(preview(&desk, &writer.secret, &body).await);
    assert_eq!(receipt.as_object().unwrap().len(), 2);
    let token = receipt["deletion_plan"]["confirmation"].as_str().unwrap();
    assert_eq!(token.len(), 395);
    let raw = serde_json::to_string(&receipt).unwrap();
    for private in [
        "Unreadable",
        "entries",
        "gaining",
        "losing",
        "versions",
        "authority_epoch",
    ] {
        assert!(!raw.contains(private));
    }
    assert_eq!(
        state(&desk).await,
        before,
        "a preview performs no document, audience or receipt write"
    );
    let again = accepted(preview(&desk, &writer.secret, &body).await);
    assert_ne!(
        again["deletion_plan"], receipt["deletion_plan"],
        "a public deterministic hash would expose a guessing oracle"
    );
    let reader = key(&desk, desk.ada.id(), scope(root, true)).await;
    let full = accepted(preview(&desk, &reader.secret, &body).await);
    let ordinary = accepted(preview(&desk, &desk.ada.token, &body).await);
    assert_eq!(full, ordinary);
    assert_eq!(full["stamp"]["entries"].as_array().unwrap().len(), 2);
    assert!(
        full["stamp"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["item"]["id"] == json!(first))
    );
    let mut one = body.clone();
    one.items = vec![DocumentTreeItem::Document(first)];
    let exact = key(
        &desk,
        desk.ada.id(),
        vec![rule(OP, "document", first, "self", &["manage", "read"])],
    )
    .await;
    let one = accepted(preview(&desk, &exact.secret, &one).await);
    assert_eq!(one["stamp"]["entries"].as_array().unwrap().len(), 1);
    assert_eq!(one["stamp"]["entries"][0]["item"]["id"], json!(first));
    child(
        &desk,
        root,
        "A longer hidden name with another membership row",
    )
    .await;
    let wider = accepted(preview(&desk, &writer.secret, &body).await);
    assert_eq!(
        wider["deletion_plan"]["confirmation"]
            .as_str()
            .unwrap()
            .len(),
        token.len(),
        "wire size reveals neither membership nor title length"
    );
}

#[tokio::test]
async fn protected_confirmation_uses_the_native_writer_and_refuses_tampering_replacement_and_stale_membership()
 {
    let desk = Desk::new().await;
    let root = folder(&desk, "Confirmed native tree").await;
    let leaf = child(&desk, root, "Confirmed child").await;
    let key = key(&desk, desk.ada.id(), scope(root, false)).await;
    let mut body = body(root);
    let value = accepted(preview(&desk, &key.secret, &body).await);
    body.deletion_plan = Some(confirmation(&value));
    let before = state(&desk).await;
    let mut tampered = body.clone();
    if let Some(DocumentTreeDeletion::Sealed(sealed)) = &mut tampered.deletion_plan {
        let first = if sealed.confirmation.starts_with('A') {
            "B"
        } else {
            "A"
        };
        sealed.confirmation.replace_range(0..1, first);
    } else {
        panic!("the restricted response must contain a protected confirmation");
    }
    assert_eq!(
        operate(&desk, &tampered).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(state(&desk).await, before);
    let other = folder(&desk, "Another actual root").await;
    let mut replaced = body.clone();
    replaced.items = vec![DocumentTreeItem::Container(other)];
    assert_eq!(operate(&desk, &replaced).await.status, StatusCode::CONFLICT);
    child(&desk, root, "Actual later member").await;
    let stale = state(&desk).await;
    assert_eq!(operate(&desk, &body).await.status, StatusCode::CONFLICT);
    assert_eq!(state(&desk).await, stale);
    body.deletion_plan = None;
    let refreshed = accepted(preview(&desk, &key.secret, &body).await);
    body.deletion_plan = Some(confirmation(&refreshed));
    let removed = accepted(operate(&desk, &body).await);
    assert_eq!(removed["entries"].as_array().unwrap().len(), 3);
    let deleted: bool =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM documents WHERE id=$1")
            .bind(leaf)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
    assert!(deleted);
    let saved = state(&desk).await;
    accepted(operate(&desk, &body).await);
    assert_eq!(
        state(&desk).await,
        saved,
        "native replay never repeats deletion, audit or its durable receipt"
    );
}

#[tokio::test]
async fn deletion_preview_checks_current_raw_expiry_after_real_scope_and_descendant_row_waits() {
    for stage in ["scope", "member"] {
        let desk = Desk::new().await;
        let root = folder(&desk, "Locked source root").await;
        let leaf = child(&desk, root, "Locked source member").await;
        let body = body(root);
        let token = key(&desk, desk.ada.id(), scope(root, true)).await;
        let before = state(&desk).await;
        let mut blocker = desk.world.db.pool.begin().await.unwrap();
        let pattern = if stage == "scope" {
            sqlx::query("SELECT node_id FROM document_tree_scopes WHERE node_id=$1 FOR UPDATE")
                .bind(desk.topic)
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            "%SELECT document_tree_lock(%"
        } else {
            sqlx::query("SELECT id FROM documents WHERE id=$1 FOR UPDATE")
                .bind(leaf)
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            "%SELECT id FROM documents WHERE id=ANY%"
        };
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = desk.world.app.clone();
        let secret = token.secret.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(DocumentTreeDeletePreview, &[])
                .token(&secret)
                .body(&body);
            request.send_bytes().await
        });
        wait_sql(&desk, pid, &request, pattern).await;
        desk.world.advance(chrono::Duration::hours(2));
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        assert_eq!(state(&desk).await, before);
    }
}

#[tokio::test]
async fn held_preview_publication_keeps_its_original_plan_or_protected_confirmation_without_replanning()
 {
    for change in ["none", "read_closed", "expired", "membership", "authority"] {
        let desk = Desk::new().await;
        let root = folder(&desk, "Held preview root").await;
        let leaf = child(&desk, root, "Held preview member").await;
        let body = body(root);
        let token = key(&desk, desk.ada.id(), scope(root, true)).await;
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
        let secret = token.secret.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(DocumentTreeDeletePreview, &[])
                .token(&secret)
                .body(&body);
            request.send_bytes().await
        });
        wait_sql(&desk, pid, &request, "%INSERT INTO api_token_activity%").await;
        if change == "expired" {
            desk.world.advance(chrono::Duration::hours(2));
        }
        if change == "read_closed" {
            let current: KeyScope =
                serde_json::from_value(json!({"rules":scope(root, false)})).unwrap();
            assert!(current.validate().is_empty());
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(token.id)
                .bind(json!(current))
                .execute(&desk.world.db.pool)
                .await
                .unwrap();
        }
        if change == "membership" {
            child(&desk, root, "Changed after preview").await;
        }
        if change == "authority" {
            let mut tx = desk.world.db.pool.begin().await.unwrap();
            // Keep the same ordinary audience but replace a genuine stored grant identity.
            let old: Id = sqlx::query_scalar("SELECT id FROM permission_entries WHERE object_kind='document' AND object_id=$1 ORDER BY id LIMIT 1").bind(leaf).fetch_one(&mut *tx).await.unwrap();
            sqlx::query("UPDATE permission_entries SET id=$2 WHERE id=$1")
                .bind(old)
                .bind(new_id())
                .execute(&mut *tx)
                .await
                .unwrap();
            orch_db::queries::access::bump_epoch(&mut tx).await.unwrap();
            tx.commit().await.unwrap();
        }
        let expected = state(&desk).await;
        blocker.commit().await.unwrap();
        let value = accepted(tokio::time::timeout(WAIT, request).await.unwrap().unwrap());
        if change == "none" {
            assert_eq!(value["stamp"]["entries"].as_array().unwrap().len(), 2);
        } else {
            let _protected = confirmation(&value);
            assert!(value.get("stamp").is_none());
        }
        assert_eq!(
            state(&desk).await,
            expected,
            "publication never recomputes an audience into a replacement plan or performs deletion"
        );
    }
}

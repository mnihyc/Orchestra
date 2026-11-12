//! Native compound movement preserves its effect independently of completed content Read.
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
use orch_api_contract::documents::{DocumentMove, DocumentMoveBody};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use serde_json::json;
fn rule(operation: &str, kind: &str, id: Id, reach: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":actions}]})
}
fn rules(document: Id, to: Id, read: bool) -> Vec<Json> {
    vec![
        rule("DocumentMove", "document", document, "self", &["manage"]),
        rule(
            "DocumentMove",
            "topic",
            to,
            "subtree",
            if read { &["write", "read"] } else { &["write"] },
        ),
    ]
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
            name: "Native document movement",
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
async fn move_to(desk: &Desk, token: &str, document: Id, to: Id) -> RawReply {
    let request = desk
        .world
        .app
        .call(DocumentMove, &[&document])
        .token(token)
        .body(&DocumentMoveBody { node_id: to });
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
async fn saved(desk: &Desk, document: Id) -> (Id, String, i64) {
    sqlx::query_as("SELECT node_id,xmin::text,doc_seq FROM documents WHERE id=$1")
        .bind(document)
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap()
}
async fn audits(desk: &Desk, document: Id) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='document.moved'",
    )
    .bind(document)
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap()
}
#[tokio::test]
async fn document_move_manage_and_write_keep_the_effect_without_borrowing_another_read() {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Write-only moved document")
        .await
        .base
        .id;
    let to = desk.another_topic("Actual destination").await;
    let before = saved(&desk, document).await;
    let mut grants = rules(document, to, false);
    grants.push(rule(
        "DocumentGet.read",
        "document",
        document,
        "self",
        &["read"],
    ));
    let token = key(&desk, desk.ada.id(), grants).await;
    let value = accepted(move_to(&desk, &token, document, to).await);
    assert_eq!(
        value,
        json!({"projection":"receipt","document_id":document,"node_id":to})
    );
    let after = saved(&desk, document).await;
    assert_eq!(after.0, to);
    assert!(after.2 > before.2);
    assert_ne!(after.1, before.1);
    assert_eq!(audits(&desk, document).await, 1);
    let other = desk
        .make(&desk.ada, "Missing real destination permission")
        .await
        .base
        .id;
    let missing = key(
        &desk,
        desk.ada.id(),
        vec![rule(
            "DocumentMove",
            "document",
            other,
            "self",
            &["manage", "read"],
        )],
    )
    .await;
    assert_eq!(
        move_to(&desk, &missing, other, to).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(saved(&desk, other).await.0, desk.topic);
}
#[tokio::test]
async fn document_move_completed_read_uses_the_actual_post_move_governor_and_native_document() {
    let desk = Desk::new().await;
    let to = desk.another_topic("Read at actual destination").await;
    let first = desk
        .make(&desk.ada, "Readable native moved content")
        .await
        .base
        .id;
    let token = key(&desk, desk.ada.id(), rules(first, to, true)).await;
    let full = accepted(move_to(&desk, &token, first, to).await);
    assert!(full.get("projection").is_none());
    assert_eq!(full["id"], json!(first));
    assert_eq!(full["title"], "Readable native moved content");
    assert_eq!(full["node"]["id"], json!(to));
    let second = desk
        .make(&desk.ada, "Old source read is not current result read")
        .await
        .base
        .id;
    let mut grants = rules(second, to, false);
    grants.push(rule(
        "DocumentMove",
        "topic",
        desk.topic,
        "subtree",
        &["read"],
    ));
    let old = key(&desk, desk.ada.id(), grants).await;
    assert_eq!(
        accepted(move_to(&desk, &old, second, to).await),
        json!({"projection":"receipt","document_id":second,"node_id":to})
    );
    assert_eq!(saved(&desk, second).await.0, to);
    assert_eq!(audits(&desk, second).await, 1);
    let third = desk
        .make(&desk.ada, "Completed management is not admitted again")
        .await
        .base
        .id;
    let directional = key(
        &desk,
        desk.ada.id(),
        vec![
            rule("DocumentMove", "topic", desk.topic, "subtree", &["manage"]),
            rule("DocumentMove", "topic", to, "subtree", &["write", "read"]),
        ],
    )
    .await;
    let value = accepted(move_to(&desk, &directional, third, to).await);
    assert!(
        value.get("projection").is_none(),
        "current result Read cannot require the old source management scope again"
    );
    assert_eq!(value["node"]["id"], json!(to));
    assert_eq!(audits(&desk, third).await, 1);
}
#[tokio::test]
async fn document_move_noop_keeps_the_original_row_and_audit_with_a_truthful_receipt() {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Already in the native node directory")
        .await
        .base
        .id;
    let before = saved(&desk, document).await;
    let token = key(&desk, desk.ada.id(), rules(document, desk.topic, false)).await;
    assert_eq!(
        accepted(move_to(&desk, &token, document, desk.topic).await),
        json!({"projection":"receipt","document_id":document,"node_id":desk.topic})
    );
    assert_eq!(saved(&desk, document).await, before);
    assert_eq!(audits(&desk, document).await, 0);
}
#[tokio::test]
async fn document_move_keeps_native_management_and_typed_destination_constraints() {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Native constrained move")
        .await
        .base
        .id;
    let to = desk.another_topic("Constrained destination").await;
    let viewer = desk.member("move-viewer", "viewer").await;
    let denied = key(&desk, viewer.id(), rules(document, to, true)).await;
    assert_eq!(
        move_to(&desk, &denied, document, to).await.status,
        StatusCode::FORBIDDEN
    );
    let source = key(
        &desk,
        desk.ada.id(),
        vec![
            rule(
                "DocumentMove",
                "document_container",
                document,
                "self",
                &["manage", "read"],
            ),
            rule("DocumentMove", "topic", to, "self", &["write"]),
        ],
    )
    .await;
    assert_eq!(
        move_to(&desk, &source, document, to).await.status,
        StatusCode::FORBIDDEN
    );
    let destination = key(
        &desk,
        desk.ada.id(),
        vec![
            rule(
                "DocumentMove",
                "document",
                document,
                "self",
                &["manage", "read"],
            ),
            rule("DocumentMove", "document", to, "self", &["write"]),
        ],
    )
    .await;
    assert_eq!(
        move_to(&desk, &destination, document, to).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(saved(&desk, document).await.0, desk.topic);
    assert_eq!(audits(&desk, document).await, 0);
}
#[tokio::test]
async fn document_move_rechecks_expiry_after_its_actual_source_row_wait_before_update() {
    for expire in [false, true] {
        let desk = Desk::new().await;
        let document = desk
            .make(&desk.ada, "Waiting native movement")
            .await
            .base
            .id;
        let to = desk.another_topic("Waiting move destination").await;
        let before = saved(&desk, document).await;
        let token = key(&desk, desk.ada.id(), rules(document, to, false)).await;
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
            let request = app
                .call(DocumentMove, &[&document])
                .token(&token)
                .body(&DocumentMoveBody { node_id: to });
            request.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async {loop {
            assert!(!request.is_finished(),"the actual native movement row wait was not reached");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM documents%' AND query LIKE '%FOR UPDATE%')")
                .bind(pid).fetch_one(&desk.world.db.pool).await.unwrap();
            if blocked {break;}tokio::task::yield_now().await;
        }}).await.expect("the actual movement source row must wait");
        if expire {
            desk.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let result = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        if expire {
            assert_eq!(result.status, StatusCode::UNAUTHORIZED);
            assert_eq!(saved(&desk, document).await, before);
            assert_eq!(audits(&desk, document).await, 0);
        } else {
            assert_eq!(
                accepted(result),
                json!({"projection":"receipt","document_id":document,"node_id":to})
            );
            assert_eq!(saved(&desk, document).await.0, to);
            assert_eq!(audits(&desk, document).await, 1);
        }
    }
}
#[tokio::test]
async fn document_move_acquires_the_real_readme_cleanup_lock_before_its_first_update() {
    for expire in [false, true] {
        let desk = Desk::new().await;
        let document = desk.make(&desk.ada, "Actual readme movement").await.base.id;
        let to = desk.another_topic("Readme movement destination").await;
        sqlx::query("UPDATE nodes SET readme_document_id=$2 WHERE id=$1")
            .bind(desk.topic)
            .bind(document)
            .execute(&desk.world.db.pool)
            .await
            .unwrap();
        let before = saved(&desk, document).await;
        let token = key(&desk, desk.ada.id(), rules(document, to, false)).await;
        let mut blocker = desk.world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR KEY SHARE")
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
            let request = app
                .call(DocumentMove, &[&document])
                .token(&token)
                .body(&DocumentMoveBody { node_id: to });
            request.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async {loop {
            assert!(!request.is_finished(),"the actual readme cleanup footprint was not reached");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%readme_document_id%' AND query LIKE '%ORDER BY id FOR UPDATE%')")
                .bind(pid).fetch_one(&desk.world.db.pool).await.unwrap();
            if blocked {break;}tokio::task::yield_now().await;
        }}).await.expect("the existing native readme relation must be locked during preparation");
        assert_eq!(
            saved(&desk, document).await,
            before,
            "the first document update must not precede this native wait"
        );
        if expire {
            desk.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let result = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        let readme: Option<Id> =
            sqlx::query_scalar("SELECT readme_document_id FROM nodes WHERE id=$1")
                .bind(desk.topic)
                .fetch_one(&desk.world.db.pool)
                .await
                .unwrap();
        if expire {
            assert_eq!(result.status, StatusCode::UNAUTHORIZED);
            assert_eq!(saved(&desk, document).await, before);
            assert_eq!(readme, Some(document));
        } else {
            accepted(result);
            assert_eq!(saved(&desk, document).await.0, to);
            assert_eq!(readme, None);
        }
    }
}
#[tokio::test]
async fn document_move_publication_preserves_the_committed_effect_and_never_replays_the_writer() {
    for change in ["unchanged", "expired", "read_lost"] {
        let desk = Desk::new().await;
        let document = desk
            .make(&desk.ada, "Held committed movement")
            .await
            .base
            .id;
        let to = desk.another_topic("Committed destination").await;
        let replacement = desk.another_topic("Later actual location").await;
        let token = key(&desk, desk.ada.id(), rules(document, to, true)).await;
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
                .call(DocumentMove, &[&document])
                .token(&token)
                .body(&DocumentMoveBody { node_id: to });
            request.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async {loop {
            assert!(!request.is_finished(),"the committed native movement did not reach final bookkeeping");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&desk.world.db.pool).await.unwrap();
            if blocked {break;}tokio::task::yield_now().await;
        }}).await.expect("the actual committed movement reply must wait at activity append");
        assert_eq!(saved(&desk, document).await.0, to);
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
            assert_eq!(value["node"]["id"], json!(to));
        } else {
            assert_eq!(
                value,
                json!({"projection":"receipt","document_id":document,"node_id":to})
            );
        }
        assert_eq!(saved(&desk, document).await, committed);
        assert_eq!(audits(&desk, document).await, 1);
    }
}

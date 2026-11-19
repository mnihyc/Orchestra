//! Native compound audience review has independent disclosure and never performs a move.
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
use orch_api_contract::access::RoleDelete;
use orch_api_contract::documents::{DocumentMoveBody, DocumentMovePreview};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::NoContentReply;
fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]})
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
            name: "Native move review",
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
fn rules(document: Id, to: Id, read: bool) -> Vec<Json> {
    vec![
        rule(
            "DocumentMovePreview",
            "document",
            document,
            if read {
                &["manage", "read"]
            } else {
                &["manage"]
            },
        ),
        rule("DocumentMovePreview", "topic", to, &["write"]),
    ]
}
async fn preview(desk: &Desk, token: &str, document: Id, to: Id) -> RawReply {
    let request = desk
        .world
        .app
        .call(DocumentMovePreview, &[&document])
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
async fn destination(desk: &Desk) -> (Id, places::Who, Id) {
    let to = desk.another_topic("Native audience destination").await;
    let person = desk.stranger("review-recipient").await;
    places::give_role(&desk.world, to, &person, "viewer", &desk.ada).await;
    let role:Id=sqlx::query_scalar("SELECT id FROM node_roles WHERE node_id=$1 AND subject_id=$2 AND role='viewer' AND removed_at IS NULL")
        .bind(to).bind(person.id()).fetch_one(&desk.world.db.pool).await.unwrap();
    (to, person, role)
}
#[tokio::test]
async fn move_review_manage_and_write_complete_without_borrowing_another_operations_read() {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Native reviewed document")
        .await
        .base
        .id;
    let (to, _, _) = destination(&desk).await;
    let before = saved(&desk, document).await;
    let mut allowed = rules(document, to, false);
    allowed.push(rule("DocumentGet.read", "document", document, &["read"]));
    let token = key(&desk, desk.ada.id(), allowed).await;
    let value = accepted(preview(&desk, &token, document, to).await);
    assert_eq!(
        value,
        json!({"projection":"receipt","document_id":document,"node_id":to})
    );
    assert_eq!(saved(&desk, document).await, before);
    assert_eq!(audits(&desk, document).await, 0);
    let no_destination = key(
        &desk,
        desk.ada.id(),
        vec![rule(
            "DocumentMovePreview",
            "document",
            document,
            &["manage", "read"],
        )],
    )
    .await;
    assert_eq!(
        preview(&desk, &no_destination, document, to).await.status,
        StatusCode::FORBIDDEN
    );
}
#[tokio::test]
async fn move_review_own_read_keeps_the_actual_native_audience_and_does_not_move() {
    let desk = Desk::new().await;
    let document = desk.make(&desk.ada, "Same native audience").await.base.id;
    let (to, person, _) = destination(&desk).await;
    let before = saved(&desk, document).await;
    let ordinary = accepted(preview(&desk, &desk.ada.token, document, to).await);
    assert!(
        ordinary["gaining"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["user"]["id"] == json!(person.id()))
    );
    let token = key(&desk, desk.ada.id(), rules(document, to, true)).await;
    let keyed = accepted(preview(&desk, &token, document, to).await);
    assert_eq!(keyed, ordinary);
    assert_eq!(saved(&desk, document).await, before);
    assert_eq!(audits(&desk, document).await, 0);
    let viewer = desk.member("review-viewer", "viewer").await;
    let ordinary_denial = key(&desk, viewer.id(), rules(document, to, true)).await;
    assert_eq!(
        preview(&desk, &ordinary_denial, document, to).await.status,
        StatusCode::FORBIDDEN
    );
    let wrong_source = key(
        &desk,
        desk.ada.id(),
        vec![
            rule(
                "DocumentMovePreview",
                "document_container",
                document,
                &["manage", "read"],
            ),
            rule("DocumentMovePreview", "topic", to, &["write"]),
        ],
    )
    .await;
    assert_eq!(
        preview(&desk, &wrong_source, document, to).await.status,
        StatusCode::FORBIDDEN
    );
    let wrong_destination = key(
        &desk,
        desk.ada.id(),
        vec![
            rule(
                "DocumentMovePreview",
                "document",
                document,
                &["manage", "read"],
            ),
            rule("DocumentMovePreview", "document", to, &["write"]),
        ],
    )
    .await;
    assert_eq!(
        preview(&desk, &wrong_destination, document, to)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}
#[tokio::test]
async fn move_review_same_directory_noop_still_uses_its_real_compound_admission_and_receipt() {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Already in its native directory")
        .await
        .base
        .id;
    let before = saved(&desk, document).await;
    let token = key(&desk, desk.ada.id(), rules(document, desk.topic, false)).await;
    assert_eq!(
        accepted(preview(&desk, &token, document, desk.topic).await),
        json!({"projection":"receipt","document_id":document,"node_id":desk.topic})
    );
    let token = key(&desk, desk.ada.id(), rules(document, desk.topic, true)).await;
    let value = accepted(preview(&desk, &token, document, desk.topic).await);
    assert_eq!(value["pages"], 1);
    assert_eq!(value["gaining"], json!([]));
    assert_eq!(value["losing"], json!([]));
    assert_eq!(saved(&desk, document).await, before);
    assert_eq!(audits(&desk, document).await, 0);
}
#[tokio::test]
async fn move_review_rechecks_key_expiry_after_the_actual_source_row_wait() {
    for expire in [false, true] {
        let desk = Desk::new().await;
        let document = desk
            .make(&desk.ada, "Waiting reviewed source")
            .await
            .base
            .id;
        let (to, _, _) = destination(&desk).await;
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
                .call(DocumentMovePreview, &[&document])
                .token(&token)
                .body(&DocumentMoveBody { node_id: to });
            request.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async {
            loop {
                assert!(!request.is_finished(),"the native source row wait was not reached");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM documents%' AND query LIKE '%FOR UPDATE%')")
                    .bind(pid).fetch_one(&desk.world.db.pool).await.unwrap();
                if blocked {break;}tokio::task::yield_now().await;
            }
        }).await.expect("the actual review source row must wait");
        if expire {
            desk.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        } else {
            assert_eq!(
                accepted(reply),
                json!({"projection":"receipt","document_id":document,"node_id":to})
            );
        }
        assert_eq!(saved(&desk, document).await, before);
        assert_eq!(audits(&desk, document).await, 0);
    }
}
#[tokio::test]
async fn move_review_publication_keeps_the_original_plan_and_closes_after_real_activity_wait() {
    for change in ["unchanged", "expired", "audience"] {
        let desk = Desk::new().await;
        let document = desk.make(&desk.ada, "Held native review").await.base.id;
        let (to, person, role) = destination(&desk).await;
        let before = saved(&desk, document).await;
        let token = key(&desk, desk.ada.id(), rules(document, to, true)).await;
        let control = accepted(preview(&desk, &token, document, to).await);
        assert!(
            control["gaining"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["user"]["id"] == json!(person.id()))
        );
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
        let request_token = token.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(DocumentMovePreview, &[&document])
                .token(&request_token)
                .body(&DocumentMoveBody { node_id: to });
            request.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async {
            loop {
                assert!(!request.is_finished(),"the accepted native review did not reach final activity bookkeeping");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&desk.world.db.pool).await.unwrap();
                if blocked {break;}tokio::task::yield_now().await;
            }
        }).await.expect("the actual native preview reply must wait at activity append");
        if change == "expired" {
            desk.world.advance(chrono::Duration::hours(2));
        }
        if change == "audience" {
            {
                let request = desk
                    .world
                    .app
                    .call(RoleDelete, &[&role])
                    .token(&desk.ada.token);
                request.send().await
            }
            .no_content();
        }
        blocker.commit().await.unwrap();
        let result = accepted(tokio::time::timeout(wait, request).await.unwrap().unwrap());
        if change == "unchanged" {
            assert_eq!(result, control);
        } else {
            assert_eq!(
                result,
                json!({"projection":"receipt","document_id":document,"node_id":to})
            );
        }
        assert_eq!(saved(&desk, document).await, before);
        assert_eq!(audits(&desk, document).await, 0);
        if change == "audience" {
            let current = accepted(preview(&desk, &token, document, to).await);
            assert!(current.get("projection").is_none());
            assert!(
                !current["gaining"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["user"]["id"] == json!(person.id()))
            );
        }
    }
}

#[tokio::test]
async fn move_review_legacy_manage_and_create_key_keeps_its_native_generated_audience() {
    let desk = Desk::new().await;
    let document = desk.make(&desk.ada, "Legacy native review").await.base.id;
    let (to, person, _) = destination(&desk).await;
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: desk.ada.id(),
            name: "Legacy compound native review",
            token_hash: &token_hash(&secret),
            scopes: &["doc.manage".into(), "doc.create".into()],
            node_id: None,
            at: desk.world.now(),
            expires_at: Some(desk.world.now() + chrono::Duration::hours(1)),
        },
    )
    .await
    .unwrap();
    let value = accepted(preview(&desk, &secret, document, to).await);
    assert!(value.get("projection").is_none());
    assert!(
        value["gaining"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["user"]["id"] == json!(person.id()))
    );
    assert_eq!(saved(&desk, document).await.0, desk.topic);
    assert_eq!(audits(&desk, document).await, 0);
}

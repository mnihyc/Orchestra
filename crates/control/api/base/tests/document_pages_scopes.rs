//! Native first-page aggregation keeps its own operation and every producing page's sources.
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
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    NodeDocumentPage, NodeDocumentPagesGet, NodeDocumentPagesGetQuery,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use serde_json::json;
fn rule(operation: &str, kind: &str, id: Id, reach: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":["read"]}]})
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
            name: "Native aggregate document pages",
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
fn query(nodes: &[Id], limit: Option<u32>) -> NodeDocumentPagesGetQuery {
    NodeDocumentPagesGetQuery {
        node_ids: nodes
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(","),
        limit,
    }
}
async fn pages(desk: &Desk, token: &str, nodes: &[Id], limit: Option<u32>) -> RawReply {
    let request = desk
        .world
        .app
        .call(NodeDocumentPagesGet, &[&nodes[0]])
        .token(token)
        .query(&query(nodes, limit));
    request.send_bytes().await
}
fn accepted(reply: RawReply) -> Vec<NodeDocumentPage> {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn ids(page: &NodeDocumentPage) -> Vec<Id> {
    page.page
        .as_ref()
        .unwrap()
        .items
        .iter()
        .filter_map(|item| item.as_visible().map(|row| row.id))
        .collect()
}
#[tokio::test]
async fn aggregate_pages_use_their_own_operation_and_actual_sources_without_child_operation_borrowing()
 {
    let desk = Desk::new().await;
    let other = desk.another_topic("Another native page").await;
    let first = desk
        .make(&desk.ada, "First selected native document")
        .await
        .base
        .id;
    let second = desk
        .make_at(
            &desk.ada,
            other,
            desk::body("Second selected native document"),
        )
        .await
        .ok()
        .base
        .id;
    desk.make(&desk.ada, "Unselected first page document").await;
    desk.make_at(
        &desk.ada,
        other,
        desk::body("Unselected second page document"),
    )
    .await
    .ok();
    let token = key(
        &desk,
        desk.ada.id(),
        vec![
            rule("NodeDocumentPagesGet", "document", first, "self"),
            rule("NodeDocumentPagesGet", "document", second, "self"),
        ],
    )
    .await;
    let response = accepted(pages(&desk, &token, &[desk.topic, other], Some(5)).await);
    assert_eq!(response.len(), 2);
    assert_eq!(ids(&response[0]), vec![first]);
    assert_eq!(ids(&response[1]), vec![second]);
    assert!(response.iter().all(|part| part.error.is_none()));
    let text = serde_json::to_string(&response).unwrap();
    assert!(!text.contains("Unselected first page document"));
    assert!(!text.contains("Unselected second page document"));
    let child = key(
        &desk,
        desk.ada.id(),
        vec![rule("NodeDocumentList.read", "document", first, "self")],
    )
    .await;
    assert_eq!(
        pages(&desk, &child, &[desk.topic], Some(5)).await.status,
        StatusCode::FORBIDDEN
    );
    let one = key(
        &desk,
        desk.ada.id(),
        vec![rule("NodeDocumentPagesGet", "document", first, "self")],
    )
    .await;
    let response = accepted(pages(&desk, &one, &[desk.topic, other], Some(5)).await);
    assert_eq!(ids(&response[0]), vec![first]);
    assert!(ids(&response[1]).is_empty());
}
#[tokio::test]
async fn aggregate_pages_live_labels_need_read_from_the_aggregate_operation() {
    let desk = Desk::new().await;
    let document = desk.make(&desk.ada, "Own aggregate content").await.base.id;
    let token = key(
        &desk,
        desk.ada.id(),
        vec![
            rule("NodeDocumentPagesGet", "document", document, "self"),
            rule("NodeDocumentList.read", "org", desk.org, "subtree"),
        ],
    )
    .await;
    let hidden = accepted(pages(&desk, &token, &[desk.topic], None).await);
    assert_eq!(ids(&hidden[0]), vec![document]);
    let text = serde_json::to_string(&hidden).unwrap();
    assert!(!text.contains("Acme"));
    assert!(!text.contains("Plans"));
    let full = key(
        &desk,
        desk.ada.id(),
        vec![
            rule("NodeDocumentPagesGet", "document", document, "self"),
            rule("NodeDocumentPagesGet", "org", desk.org, "subtree"),
        ],
    )
    .await;
    let visible = accepted(pages(&desk, &full, &[desk.topic], None).await);
    assert_eq!(ids(&visible[0]), vec![document]);
    let ordinary = accepted(pages(&desk, &desk.ada.token, &[desk.topic], None).await);
    let node = |page: &NodeDocumentPage| {
        page.page.as_ref().unwrap().items[0]
            .as_visible()
            .unwrap()
            .node
            .clone()
    };
    assert_eq!(node(&ordinary[0]).name.as_deref(), Some("Plans"));
    assert_eq!(node(&visible[0]), node(&ordinary[0]));
}
#[tokio::test]
async fn aggregate_pages_filter_before_each_native_window_and_preserve_the_requested_order() {
    let desk = Desk::new().await;
    let other = desk.another_topic("Second window").await;
    let first = desk.make(&desk.ada, "Older permitted first").await.base.id;
    let second = desk
        .make_at(&desk.ada, other, desk::body("Older permitted second"))
        .await
        .ok()
        .base
        .id;
    for index in 0..5 {
        desk.world.advance(chrono::Duration::seconds(1));
        desk.make(&desk.ada, &format!("Newer excluded first {index}"))
            .await;
        desk.make_at(
            &desk.ada,
            other,
            desk::body(&format!("Newer excluded second {index}")),
        )
        .await
        .ok();
    }
    let token = key(
        &desk,
        desk.ada.id(),
        vec![
            rule("NodeDocumentPagesGet", "document", first, "self"),
            rule("NodeDocumentPagesGet", "document", second, "self"),
        ],
    )
    .await;
    let response = accepted(pages(&desk, &token, &[desk.topic, other, desk.topic], Some(1)).await);
    assert_eq!(
        response.iter().map(|part| part.node_id).collect::<Vec<_>>(),
        vec![desk.topic, other, desk.topic]
    );
    assert_eq!(ids(&response[0]), vec![first]);
    assert_eq!(ids(&response[1]), vec![second]);
    assert_eq!(ids(&response[2]), vec![first]);
}
#[tokio::test]
async fn aggregate_pages_keep_real_empty_proof_and_native_member_errors_under_a_document_only_resource_filter()
 {
    let desk = Desk::new().await;
    let empty = desk.another_topic("Actually empty native page").await;
    let stranger = desk.stranger("aggregate-outsider").await;
    let foreign = places::make_org(
        &desk.world,
        &stranger,
        "Private aggregate source",
        "private-aggregate-source",
    )
    .await
    .base
    .id;
    let mut selection = rule("NodeDocumentPagesGet", "topic", empty, "subtree");
    selection["resources"] = json!(["document"]);
    let token = key(&desk, desk.ada.id(), vec![selection]).await;
    let result = accepted(pages(&desk, &token, &[empty, foreign], Some(1)).await);
    assert!(ids(&result[0]).is_empty());
    assert!(result[0].error.is_none());
    assert!(result[1].page.is_none());
    assert_eq!(
        result[1].error.as_ref().unwrap().code,
        ErrorCode::AuthzDenied
    );
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("Private aggregate source")
    );
}
#[tokio::test]
async fn aggregate_publication_rechecks_every_producing_page_after_the_actual_activity_wait() {
    for change in ["unchanged", "expired", "detached"] {
        let desk = Desk::new().await;
        let other = desk.another_topic("Other held page").await;
        let replacement = desk.another_topic("Replacement actual page").await;
        let first = desk
            .make(&desk.ada, "Held first aggregate source")
            .await
            .base
            .id;
        let second = desk
            .make_at(&desk.ada, other, desk::body("Held second aggregate source"))
            .await
            .ok()
            .base
            .id;
        let token = key(
            &desk,
            desk.ada.id(),
            vec![
                rule("NodeDocumentPagesGet", "document", first, "self"),
                rule("NodeDocumentPagesGet", "document", second, "self"),
            ],
        )
        .await;
        let control = accepted(pages(&desk, &token, &[desk.topic, other], None).await);
        assert_eq!(ids(&control[1]), vec![second]);
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
        let nodes = vec![desk.topic, other];
        let request_token = token.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(NodeDocumentPagesGet, &[&nodes[0]])
                .token(&request_token)
                .query(&query(&nodes, None));
            request.send_bytes().await
        });
        let wait = std::time::Duration::from_secs(30);
        tokio::time::timeout(wait,async {
            loop {
                assert!(!request.is_finished(),"the aggregate did not reach final activity bookkeeping");
                let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&desk.world.db.pool).await.unwrap();
                if blocked {break;}tokio::task::yield_now().await;
            }
        }).await.expect("the actual aggregate result must wait at its activity append");
        if change == "expired" {
            desk.world.advance(chrono::Duration::hours(2));
        }
        if change == "detached" {
            sqlx::query("UPDATE documents SET node_id=$2 WHERE id=$1")
                .bind(second)
                .bind(replacement)
                .execute(&desk.world.db.pool)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        if change == "unchanged" {
            let response = accepted(reply);
            assert_eq!(ids(&response[0]), vec![first]);
            assert_eq!(ids(&response[1]), vec![second]);
        } else {
            assert_eq!(
                reply.status,
                if change == "expired" {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::FORBIDDEN
                },
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            let problem: orch_api_contract::common::Problem =
                serde_json::from_slice(&reply.raw).unwrap();
            assert_eq!(
                problem.code,
                if change == "expired" {
                    ErrorCode::AuthInvalidToken
                } else {
                    ErrorCode::AuthzDenied
                }
            );
            let text = String::from_utf8_lossy(&reply.raw);
            assert!(!text.contains("Held first aggregate source"));
            assert!(!text.contains("Held second aggregate source"));
        }
        if change == "detached" {
            let current = accepted(pages(&desk, &token, &[desk.topic, replacement], None).await);
            assert_eq!(
                ids(&current[1]),
                vec![second],
                "the same stored identity and key remain valid in their actual current page"
            );
        }
    }
}

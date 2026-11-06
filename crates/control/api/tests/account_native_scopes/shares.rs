//! Direct shares keep their actual resource kind, native comparison reader and cursor window.

use super::{checked, key};
use crate::{desk::Desk, places::Who, support::FullPermissionPageReply};
use http::StatusCode;
use orch_api_contract::access::PermissionPageUpdate;
use orch_api_contract::common::PageQuery;
use orch_api_contract::document_tree::{
    DocumentContainerCreate, DocumentContainerCreateBody, DocumentTreeItem,
};
use orch_api_contract::documents::{DocumentSeen, DocumentSeenBody, NewDocumentAccess};
use orch_api_contract::users::UserSharesGet;
use orch_types::{Id, Json};
use serde_json::json;

fn rule(operation: &str, kind: &str, id: Id, action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],"allow":[{"operation":operation,"actions":[action]}]})
}
fn read(kind: &str, id: Id) -> Json {
    rule("UserSharesGet", kind, id, "read")
}
fn home(org: Id) -> Json {
    json!({"target":{"kind":"org","id":org,"reach":"subtree"},"resources":["org","project","topic","thread","document","document_container"],
        "allow":[{"operation":"UserSharesGet","actions":["read"]}]})
}
async fn give(desk: &Desk, object: &str, id: Id, to: &Who) {
    desk.world
        .app
        .call(PermissionPageUpdate, &[&object, &id])
        .token(&desk.ada.token)
        .raw_body(
            &json!({"name":[{"holders":[{"kind":"user","id":to.id()}],"choice":"full_view"}]}),
        )
        .send()
        .await
        .full_permission_page()
        .ok();
}

#[tokio::test]
async fn shared_rows_filter_scope_before_native_cursor_windows_and_never_repeat() {
    let desk = Desk::new().await;
    let bob = desk.member("shared-reader", "viewer").await;
    let mut topics = Vec::new();
    for index in 0..4 {
        let topic = desk
            .another_topic(&format!("Shared cursor row {index}"))
            .await;
        give(&desk, "nodes", topic, &bob).await;
        topics.push(topic);
    }
    topics.sort();
    let wanted = topics[3];
    let token = key(&desk.world, desk.ada.id(), vec![read("topic", wanted)]).await;
    let one = checked(
        desk.world
            .app
            .call(UserSharesGet, &[&bob.id()])
            .token(&token)
            .query(&PageQuery {
                limit: Some(1),
                cursor: None,
            })
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(one["items"].as_array().unwrap().len(), 1);
    assert_eq!(one["items"][0]["node"]["id"], json!(wanted));
    assert!(one["next_cursor"].is_null());
    let token = key(
        &desk.world,
        desk.ada.id(),
        vec![read("topic", topics[2]), read("topic", topics[3])],
    )
    .await;
    let first = checked(
        desk.world
            .app
            .call(UserSharesGet, &[&bob.id()])
            .token(&token)
            .query(&PageQuery {
                limit: Some(1),
                cursor: None,
            })
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    let cursor = first["next_cursor"]
        .as_str()
        .expect("the allowed look-ahead supplies a real cursor")
        .to_owned();
    let second = checked(
        desk.world
            .app
            .call(UserSharesGet, &[&bob.id()])
            .token(&token)
            .query(&PageQuery {
                limit: Some(1),
                cursor: Some(cursor),
            })
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(first["items"][0]["node"]["id"], json!(topics[2]));
    assert_eq!(second["items"][0]["node"]["id"], json!(topics[3]));
    assert!(second["next_cursor"].is_null());
}

#[tokio::test]
async fn shared_folder_uses_its_actual_kind_without_inventing_a_place_or_account_read() {
    let desk = Desk::new().await;
    let bob = desk.stranger("folder-reader").await;
    let made = desk
        .world
        .app
        .call(DocumentContainerCreate, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&DocumentContainerCreateBody {
            parent_container_id: None,
            name: "Direct scoped folder".into(),
            client_id: "scoped-shared-folder".into(),
        })
        .send()
        .await
        .ok();
    let folder = match made.entries.first().map(|entry| entry.item) {
        Some(DocumentTreeItem::Container(id)) => id,
        _ => panic!("the native folder receipt must exist"),
    };
    give(&desk, "document-containers", folder, &bob).await;
    let token = key(
        &desk.world,
        bob.id(),
        vec![read("document_container", folder)],
    )
    .await;
    let body = checked(
        desk.world
            .app
            .call(UserSharesGet, &[&desk.ada.id()])
            .token(&token)
            .query(&PageQuery::default())
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["folder"]["id"], json!(folder));
    assert_eq!(body["items"][0]["folder"]["name"], "Direct scoped folder");
    let wrong = key(&desk.world, bob.id(), vec![read("document", folder)]).await;
    let empty = checked(
        desk.world
            .app
            .call(UserSharesGet, &[&desk.ada.id()])
            .token(&wrong)
            .query(&PageQuery::default())
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(empty["items"], json!([]));
    for invalid in [
        rule("ProfileGet", "document_container", folder, "read"),
        rule("UserSharesGet", "document_container", folder, "manage"),
    ] {
        let token = key(&desk.world, bob.id(), vec![invalid]).await;
        checked(
            desk.world
                .app
                .call(UserSharesGet, &[&desk.ada.id()])
                .token(&token)
                .query(&PageQuery::default())
                .send_bytes()
                .await,
            StatusCode::FORBIDDEN,
        );
    }
}

#[tokio::test]
async fn shared_document_names_and_personal_fields_use_the_request_actor_not_the_comparison() {
    let desk = Desk::new().await;
    let bob = desk.stranger("document-reader").await;
    let mut body = crate::desk::body("Scoped direct document");
    body.access = Some(NewDocumentAccess::Private);
    let doc = desk.make_body(&desk.ada, body).await.ok();
    give(&desk, "documents", doc.base.id, &bob).await;
    desk.world
        .app
        .call(DocumentSeen, &[&doc.base.id])
        .token(&desk.ada.token)
        .body(&DocumentSeenBody { seq: doc.base.seq })
        .send()
        .await
        .ok();
    let token = key(
        &desk.world,
        desk.ada.id(),
        vec![
            read("document", doc.base.id),
            rule("NodeGet.read", "topic", desk.topic, "read"),
        ],
    )
    .await;
    let cut = checked(
        desk.world
            .app
            .call(UserSharesGet, &[&bob.id()])
            .token(&token)
            .query(&PageQuery::default())
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(
        cut["items"][0]["document"]["title"],
        "Scoped direct document"
    );
    assert_eq!(cut["items"][0]["document"]["seen_seq"], json!(doc.base.seq));
    assert!(cut["items"][0]["document"]["node"]["name"].is_null());
    let token = key(&desk.world, desk.ada.id(), vec![home(desk.org)]).await;
    let full = checked(
        desk.world
            .app
            .call(UserSharesGet, &[&bob.id()])
            .token(&token)
            .query(&PageQuery::default())
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    let native = desk
        .world
        .app
        .call(UserSharesGet, &[&bob.id()])
        .token(&desk.ada.token)
        .query(&PageQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(full, serde_json::to_value(native).unwrap());
    assert_eq!(full["items"][0]["document"]["node"]["name"], "Plans");
    let stranger = desk.stranger("uninvolved-reader").await;
    let token = key(
        &desk.world,
        stranger.id(),
        vec![read("document", doc.base.id)],
    )
    .await;
    let absent = checked(
        desk.world
            .app
            .call(UserSharesGet, &[&bob.id()])
            .token(&token)
            .query(&PageQuery::default())
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(absent["items"], json!([]));
}

#[tokio::test]
async fn shared_empty_self_and_native_cursor_errors_keep_their_original_shapes() {
    let desk = Desk::new().await;
    let token = key(&desk.world, desk.ada.id(), vec![read("topic", desk.topic)]).await;
    let own = checked(
        desk.world
            .app
            .call(UserSharesGet, &[&desk.ada.id()])
            .token(&token)
            .query(&PageQuery::default())
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(own["items"], json!([]));
    assert!(own["next_cursor"].is_null());
    checked(
        desk.world
            .app
            .call(UserSharesGet, &[&desk.ada.id()])
            .token(&token)
            .query(&PageQuery {
                limit: None,
                cursor: Some("provider|not-a-native-share".into()),
            })
            .send_bytes()
            .await,
        StatusCode::BAD_REQUEST,
    );
}

#[tokio::test]
async fn shared_publication_rechecks_the_comparison_readers_native_view_after_activity_wait() {
    let desk = Desk::new().await;
    let bob = desk.stranger("comparison-reader").await;
    let doc = desk.make(&desk.ada, "Earlier direct-share payload").await;
    give(&desk, "documents", doc.base.id, &bob).await;
    let token = key(
        &desk.world,
        desk.ada.id(),
        vec![read("document", doc.base.id)],
    )
    .await;
    let control = checked(
        desk.world
            .app
            .call(UserSharesGet, &[&bob.id()])
            .token(&token)
            .query(&PageQuery::default())
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(control["items"].as_array().unwrap().len(), 1);
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
    let other = bob.id();
    let request = tokio::spawn(async move {
        let call = app
            .call(UserSharesGet, &[&other])
            .token(&token)
            .query(&PageQuery::default());
        call.send_bytes().await
    });
    let wait = std::time::Duration::from_secs(30);
    tokio::time::timeout(wait, async {
        loop {
            assert!(!request.is_finished(), "the actual shared-page activity append must be blocked");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&desk.world.db.pool).await.unwrap();
            if blocked { break; } tokio::task::yield_now().await;
        }
    }).await.expect("the actual shared-page publication wait must be observed");
    sqlx::query("UPDATE users SET status='suspended' WHERE id=$1")
        .bind(bob.id())
        .execute(&desk.world.db.pool)
        .await
        .unwrap();
    let ordinary = desk
        .world
        .app
        .call(UserSharesGet, &[&bob.id()])
        .token(&desk.ada.token)
        .query(&PageQuery::default())
        .send()
        .await
        .ok();
    assert!(ordinary.items.is_empty());
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Earlier direct-share payload"));
}

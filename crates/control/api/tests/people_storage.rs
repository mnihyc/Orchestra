//! Paged comment history and narrow rate storage preserve authority, full exports and replay.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod desk;
mod places;
mod support;

use desk::Desk;
use orch_api_contract::access::{
    AccessChoice, HolderInput, PageChange, PageChangeName, PermissionPageUpdate,
};
use orch_api_contract::common::IDEMPOTENCY_HEADER;
use orch_api_contract::documents::{
    DocumentCommentList, DocumentCommentListQuery, DocumentDiscussionList,
    DocumentDiscussionListQuery, DocumentDiscussionResolve, DocumentDiscussionResolveBody,
    DocumentSyncGet, DocumentSyncGetQuery,
};
use orch_api_contract::notifications::{PersonNotifyBody, PersonNotifyCreate};
use orch_db::queries::{documents::discussions, notifications};
use orch_types::{Id, new_id};
use serde_json::json;
use std::collections::HashSet;

#[tokio::test]
async fn comment_pages_keep_equal_timestamps_authority_and_every_exported_reply() {
    let desk = Desk::new().await;
    let other = desk.member("other", "viewer").await;
    let outsider = desk.stranger("outsider").await;
    let document = desk.make(&desk.ada, "Paged history").await.base.id;
    let discussion = new_id();
    let now = desk.world.app.app.clock.now();
    let mut conn = desk.world.db.pool.acquire().await.unwrap();
    sqlx::query("INSERT INTO document_discussions(id,document_id,created_by,created_at,seq_updated) VALUES($1,$2,$3,$4,200)")
        .bind(discussion).bind(document).bind(desk.ada.id()).bind(now).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO document_comments(id,discussion_id,document_id,author_id,content,created_at,seq_updated)
        SELECT md5('history-comment:'||g)::uuid,$1,$2,CASE WHEN g=1 THEN $3 ELSE $4 END,
        jsonb_build_array(jsonb_build_object('type','text','text','Reply '||g)),$5::timestamptz+(g/10)*interval '1 second',g
        FROM generate_series(1,125)g")
        .bind(discussion).bind(document).bind(other.id()).bind(desk.ada.id()).bind(now).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE documents SET doc_seq=200,seq_updated=200 WHERE id=$1")
        .bind(document)
        .execute(&mut *conn)
        .await
        .unwrap();
    let expected: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM document_comments WHERE discussion_id=$1 ORDER BY created_at DESC,id DESC",
    )
    .bind(discussion)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    let sync = desk
        .world
        .app
        .call(DocumentSyncGet, &[&document])
        .token(&desk.ada.token)
        .query(&DocumentSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(sync.discussions.len(), 1);
    assert_eq!(sync.discussions[0].comments.len(), 50);
    assert!(
        !sync.discussions[0]
            .comments
            .iter()
            .any(|c| c.author.id == other.id())
    );
    let mut after = None;
    let mut all = Vec::new();
    for length in [50, 50, 25] {
        let page = desk
            .world
            .app
            .call(DocumentCommentList, &[&document, &discussion])
            .token(&other.token)
            .query(&DocumentCommentListQuery {
                limit: Some(50),
                cursor: after,
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert_eq!(page.items.len(), length);
        assert!(page.wrote, "an author on an older page can still resolve");
        all.extend(page.items.iter().rev().map(|row| row.id));
        after = page.next_cursor;
    }
    assert!(after.is_none());
    assert_eq!(all, expected);
    assert_eq!(all.iter().collect::<HashSet<_>>().len(), 125);
    desk.world
        .app
        .call(DocumentDiscussionResolve, &[&document, &discussion])
        .token(&other.token)
        .body(&DocumentDiscussionResolveBody { resolved: true })
        .send()
        .await
        .ok();
    let full_export = discussions::changed(&mut conn, document, 0, 200)
        .await
        .unwrap();
    assert_eq!(
        full_export[0].comments.len(),
        125,
        "exports keep the entire history"
    );
    assert_eq!(
        desk.world
            .app
            .call(DocumentCommentList, &[&document, &discussion])
            .token(&outsider.token)
            .query(&DocumentCommentListQuery::default())
            .send()
            .await
            .status
            .as_u16(),
        403
    );
    let another = desk.make(&desk.ada, "Other page").await.base.id;
    assert_eq!(
        desk.world
            .app
            .call(DocumentCommentList, &[&another, &discussion])
            .token(&desk.ada.token)
            .query(&DocumentCommentListQuery::default())
            .send()
            .await
            .status
            .as_u16(),
        404
    );
}

#[tokio::test]
async fn discussion_sync_windows_reset_large_deltas_and_leave_older_discussions_reachable() {
    let desk = Desk::new().await;
    let document = desk.make(&desk.ada, "Paged discussions").await.base.id;
    let mut conn = desk.world.db.pool.acquire().await.unwrap();
    sqlx::query("INSERT INTO document_discussions(id,document_id,created_by,created_at,seq_updated)
        SELECT md5('discussion-window:'||g)::uuid,$1,$2,$3::timestamptz+g*interval '1 second',g FROM generate_series(1,125)g")
        .bind(document).bind(desk.ada.id()).bind(desk.world.app.app.clock.now()).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE documents SET doc_seq=200,seq_updated=200 WHERE id=$1")
        .bind(document)
        .execute(&mut *conn)
        .await
        .unwrap();
    let sync = desk
        .world
        .app
        .call(DocumentSyncGet, &[&document])
        .token(&desk.ada.token)
        .query(&DocumentSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert!(sync.complete);
    assert_eq!(sync.discussions.len(), 50);
    let delta = desk
        .world
        .app
        .call(DocumentSyncGet, &[&document])
        .token(&desk.ada.token)
        .query(&DocumentSyncGetQuery {
            since: Some(0),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(!delta.complete);
    assert!(delta.discussions.is_empty());
    let mut cursor = None;
    let mut seen = HashSet::new();
    for length in [50, 50, 25] {
        let page = desk
            .world
            .app
            .call(DocumentDiscussionList, &[&document])
            .token(&desk.ada.token)
            .query(&DocumentDiscussionListQuery {
                limit: Some(50),
                cursor,
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert_eq!(page.items.len(), length);
        for row in page.items {
            assert!(seen.insert(row.id));
        }
        cursor = page.next_cursor;
    }
    assert!(cursor.is_none());
    assert_eq!(seen.len(), 125);
}

#[tokio::test]
async fn two_hundred_varied_recipients_fit_the_rate_index_and_replay_once() {
    let desk = Desk::new().await;
    let document = desk.make(&desk.ada, "Wide audience").await.base.id;
    desk.world
        .set("notifications.person_rate_per_hour", json!(0))
        .await;
    let mut conn = desk.world.db.pool.acquire().await.unwrap();
    let people:Vec<Id>=sqlx::query_scalar("INSERT INTO users(id,username,display_name,avatar_color,created_at)
        SELECT gen_random_uuid(),'storage_reader_'||g,'Storage reader '||g,0,$1 FROM generate_series(1,200)g RETURNING id")
        .bind(desk.world.app.app.clock.now()).fetch_all(&mut *conn).await.unwrap();
    sqlx::query(
        "INSERT INTO org_members(org_id,user_id,domain_id) SELECT $1,unnest($2::uuid[]),$1",
    )
    .bind(desk.org)
    .bind(&people)
    .execute(&mut *conn)
    .await
    .unwrap();
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &document])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::Members { id: desk.org }],
                choice: AccessChoice::FullView,
                export: Some(false),
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    for _ in 0..2 {
        desk.world
            .app
            .call(PersonNotifyCreate, &[&"documents", &document])
            .token(&desk.ada.token)
            .header(IDEMPOTENCY_HEADER, "wide-storage-event")
            .body(&PersonNotifyBody {
                user_ids: people.clone(),
                anchor: None,
                note: None,
            })
            .send()
            .await
            .ok();
    }
    let notices: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE kind='person.notify'")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(notices, 200);
    let audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='person.notified' AND person_notify_count=200",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(audit, 1);
    let used = notifications::operations::person_rate_used(
        &mut conn,
        desk.ada.id(),
        Some(desk.org),
        desk.world.app.app.clock.now() - chrono::Duration::hours(1),
    )
    .await
    .unwrap();
    assert_eq!(used, 200);
}

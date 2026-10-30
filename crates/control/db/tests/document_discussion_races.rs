//! Concurrent discussion writes preserve accepted comments and their live anchors.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_db::{
    Db,
    queries::{
        documents::{self, discussions},
        hierarchy, resources,
    },
};
use orch_testkit::{clock, database::TestDb};
use orch_types::{Id, new_id};
use serde_json::json;
use std::time::Duration;

// A local row lock should appear promptly, with time for the shared machine to schedule a waiter.
const LOCK_WAIT_LIMIT: Duration = Duration::from_secs(10);
// Poll only to observe a database waiter without keeping a processor busy.
const LOCK_POLL: Duration = Duration::from_millis(10);

async fn wait_for_lock(db: &Db, pid: i32) {
    tokio::time::timeout(LOCK_WAIT_LIMIT, async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT COALESCE((SELECT wait_event_type = 'Lock' FROM pg_stat_activity WHERE pid = $1), false)",
            )
            .bind(pid)
            .fetch_one(db.pool())
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(LOCK_POLL).await;
        }
    })
    .await
    .expect("the other write waits for this document");
}

async fn page(db: &Db) -> (Id, Id, Id) {
    let now = clock::start();
    let mut conn = db.acquire().await.unwrap();
    let author = resources::testing::insert_user(&mut conn, "ada")
        .await
        .unwrap();
    let (org, project, topic, document, block) = (new_id(), new_id(), new_id(), new_id(), new_id());
    hierarchy::testing::insert_node(&mut conn, org, "org", None, "Office", author)
        .await
        .unwrap();
    hierarchy::testing::insert_node(&mut conn, project, "project", Some(org), "Work", author)
        .await
        .unwrap();
    hierarchy::testing::insert_node(&mut conn, topic, "topic", Some(project), "Notes", author)
        .await
        .unwrap();
    documents::insert(
        &mut conn,
        &documents::NewDocument {
            id: document,
            node_id: topic,
            org_id: org,
            owner_id: author,
            created_by_thread: None,
            created_by_agent: None,
            title: "Notes",
            now,
        },
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO document_blocks (id, document_id, position, type, props, content, text, seq_created, seq_updated)
         VALUES ($1, $2, 1, 'paragraph', '{}', '[]', 'A paragraph', 0, 0)",
    )
    .bind(block)
    .bind(document)
    .execute(&mut *conn)
    .await
    .unwrap();
    (author, document, block)
}

#[tokio::test]
async fn concurrent_retries_return_the_first_discussion_and_reply_without_a_second_write() {
    let test = TestDb::fresh().await;
    let db = Db::from_pool(test.pool.clone());
    let (author, document, block) = page(&db).await;
    let now = clock::start();
    let content = json!([{ "type": "text", "text": "Once" }]);
    let first_id = new_id();
    let mut first = db.begin().await.unwrap();
    let discussions::Opened::Created(opened) = discussions::open(
        &mut first,
        document,
        block,
        "",
        author,
        &discussions::NewComment {
            id: first_id,
            content: &content,
        },
        now,
    )
    .await
    .unwrap() else {
        panic!("the discussion was created");
    };
    let discussion = opened.discussion.id;
    let (ready, started) = tokio::sync::oneshot::channel();
    let other = db.clone();
    let retry = tokio::spawn(async move {
        let mut tx = other.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        ready.send(pid).unwrap();
        let content = json!([{ "type": "text", "text": "Once" }]);
        let result = discussions::open(
            &mut tx,
            document,
            block,
            "",
            author,
            &discussions::NewComment {
                id: first_id,
                content: &content,
            },
            now,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        result
    });
    wait_for_lock(&db, started.await.unwrap()).await;
    first.commit().await.unwrap();
    let discussions::Opened::Repeated(repeated) = retry.await.unwrap() else {
        panic!("the retry returns the first discussion");
    };
    assert_eq!(repeated.discussion.id, discussion);

    let reply_id = new_id();
    let mut first = db.begin().await.unwrap();
    let discussions::Answered::Created { comment, .. } = discussions::answer(
        &mut first,
        document,
        discussion,
        author,
        &discussions::NewComment {
            id: reply_id,
            content: &content,
        },
        now,
    )
    .await
    .unwrap() else {
        panic!("the reply was created");
    };
    let (ready, started) = tokio::sync::oneshot::channel();
    let other = db.clone();
    let retry = tokio::spawn(async move {
        let mut tx = other.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        ready.send(pid).unwrap();
        let content = json!([{ "type": "text", "text": "Once" }]);
        let result = discussions::answer(
            &mut tx,
            document,
            discussion,
            author,
            &discussions::NewComment {
                id: reply_id,
                content: &content,
            },
            now,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        result
    });
    wait_for_lock(&db, started.await.unwrap()).await;
    first.commit().await.unwrap();
    let discussions::Answered::Repeated(repeated) = retry.await.unwrap() else {
        panic!("the retry returns the first reply");
    };
    assert_eq!(repeated.id, comment.id);
    let mut conn = db.acquire().await.unwrap();
    let row = documents::get(&mut conn, document).await.unwrap().unwrap();
    assert_eq!(row.doc_seq, 2);
    let full = discussions::get(&mut conn, document, discussion)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(full.comments.len(), 2);
}

#[tokio::test]
async fn a_discussion_cannot_start_on_a_block_removed_while_the_writer_waits() {
    let test = TestDb::fresh().await;
    let db = Db::from_pool(test.pool.clone());
    let (author, document, block) = page(&db).await;
    let now = clock::start();
    let mut removing = db.begin().await.unwrap();
    documents::lock(&mut removing, document)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE document_blocks SET removed_at = $2 WHERE id = $1")
        .bind(block)
        .bind(now)
        .execute(&mut *removing)
        .await
        .unwrap();
    let (ready, started) = tokio::sync::oneshot::channel();
    let other = db.clone();
    let writing = tokio::spawn(async move {
        let mut tx = other.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        ready.send(pid).unwrap();
        let content = json!([{ "type": "text", "text": "A question" }]);
        let result = discussions::open(
            &mut tx,
            document,
            block,
            "",
            author,
            &discussions::NewComment {
                id: new_id(),
                content: &content,
            },
            now,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        result
    });
    wait_for_lock(&db, started.await.unwrap()).await;
    removing.commit().await.unwrap();
    assert_eq!(writing.await.unwrap(), discussions::Opened::NoSuchBlock);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_discussions WHERE document_id = $1")
            .bind(document)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(count, 0);
}

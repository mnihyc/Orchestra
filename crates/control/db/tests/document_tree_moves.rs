//! Concurrent document moves announce the place actually left after the preceding move commits.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_db::{
    Db,
    queries::{documents, hierarchy, resources},
};
use orch_testkit::{clock, database::TestDb};
use orch_types::{Id, new_id};
use std::time::Duration;

/// A local row lock should appear promptly; shared builders get ten seconds to schedule the waiter.
const LOCK_WAIT_LIMIT: Duration = Duration::from_secs(10);
/// Poll only to observe a database waiter, at a pace that does not keep a processor busy.
const LOCK_POLL: Duration = Duration::from_millis(10);

#[tokio::test]
async fn the_second_of_two_concurrent_moves_stamps_the_place_where_the_first_left_the_document() {
    let test = TestDb::fresh().await;
    let db = Db::from_pool(test.pool.clone());
    let now = clock::start();
    let (root, first, middle, last, document) = (new_id(), new_id(), new_id(), new_id(), new_id());
    let user;
    {
        let mut conn = db.acquire().await.unwrap();
        user = resources::testing::insert_user(&mut conn, "ada")
            .await
            .unwrap();
        hierarchy::testing::insert_node(&mut conn, root, "org", None, "Office", user)
            .await
            .unwrap();
        for (id, name) in [(first, "First"), (middle, "Middle"), (last, "Last")] {
            hierarchy::testing::insert_node(&mut conn, id, "project", Some(root), name, user)
                .await
                .unwrap();
        }
        documents::insert(
            &mut conn,
            &documents::NewDocument {
                id: document,
                node_id: first,
                org_id: root,
                owner_id: user,
                created_by_thread: None,
                created_by_agent: None,
                title: "Notes",
                now,
            },
        )
        .await
        .unwrap();
    }
    // A move's places take their tree's number when it commits (`orch_commit_work`): the changes are read
    // from before both moves, and each place's number says which move announced it last.
    let before = hierarchy::tree_seq(&mut db.acquire().await.unwrap(), root)
        .await
        .unwrap();
    let mut moving = db.begin().await.unwrap();
    documents::move_to(&mut moving, document, middle, root, now)
        .await
        .unwrap();
    let (ready, started) = tokio::sync::oneshot::channel();
    let other_db = db.clone();
    let next = tokio::spawn(async move {
        let mut tx = other_db.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        ready.send(pid).unwrap();
        if let Err(error) = documents::move_to(&mut tx, document, last, root, now).await {
            // Scope-before-row ordering retries when the preceding move changed the anchor
            // while this transaction waited; a fresh transaction locks the actual new scopes.
            assert!(matches!(error, orch_db::DbError::Retry), "{error}");
            tx.rollback().await.unwrap();
            tx = other_db.begin().await.unwrap();
            documents::move_to(&mut tx, document, last, root, now)
                .await
                .unwrap();
        }
        tx.commit().await.unwrap();
    });
    let pid = started.await.unwrap();
    tokio::time::timeout(LOCK_WAIT_LIMIT, async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT COALESCE((SELECT wait_event_type = 'Lock' FROM pg_stat_activity WHERE pid = $1), false)")
                .bind(pid).fetch_one(db.pool()).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(LOCK_POLL).await;
        }
    }).await.expect("the other move waits for this document");
    moving.commit().await.unwrap();
    next.await.unwrap();
    let changes = hierarchy::tree(
        &mut db.acquire().await.unwrap(),
        root,
        Some(before),
        i64::MAX,
    )
    .await
    .unwrap();
    let place = |node: Id| {
        changes
            .iter()
            .find(|row| row.id == node)
            .map(|row| (row.docs, row.tree_seq))
            .unwrap()
    };
    let (first_docs, first_seq) = place(first);
    let (middle_docs, middle_seq) = place(middle);
    let (last_docs, last_seq) = place(last);
    assert_eq!((first_docs, middle_docs, last_docs), (0, 0, 1));
    assert_eq!(
        middle_seq, last_seq,
        "the second move announces the place it actually left"
    );
    assert!(
        first_seq < last_seq,
        "the first move already announced this place, and the second does not again"
    );
}
